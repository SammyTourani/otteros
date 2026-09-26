//! Llama forward pass with KV cache.

use alloc::vec;
use alloc::vec::Vec;
use alloc::format;
use crate::loader::{Model, DType, TensorView};
use crate::mathf;

/// Read a single f32 value from a tensor (dequantizing if Q8_0)
fn read_f32(tensor: &TensorView, idx: usize) -> f32 {
    match tensor.dtype {
        DType::F32 => {
            let off = idx * 4;
            f32::from_le_bytes([
                tensor.data[off],
                tensor.data[off + 1],
                tensor.data[off + 2],
                tensor.data[off + 3],
            ])
        }
        DType::Q8_0 => {
            let block = idx / 32;
            let lane = idx % 32;
            let block_off = block * 36;
            let qval = tensor.data[block_off + lane] as i8;
            let scale_off = block_off + 32;
            let scale = f32::from_le_bytes([
                tensor.data[scale_off],
                tensor.data[scale_off + 1],
                tensor.data[scale_off + 2],
                tensor.data[scale_off + 3],
            ]);
            (qval as f32) * scale
        }
    }
}

/// Load full tensor into f32
fn load_tensor(tensor: &TensorView, len: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(len);
    for i in 0..len {
        v.push(read_f32(tensor, i));
    }
    v
}

/// RMS norm: out[i] = x[i] / sqrt(mean(x²) + eps) * w[i]
fn rmsnorm(x: &[f32], w: &[f32], eps: f32, out: &mut [f32]) {
    let n = x.len();
    let mut sum_sq = 0.0_f64;
    for &v in x {
        let vd = v as f64;
        sum_sq += vd * vd;
    }
    let rms = mathf::sqrtf((sum_sq / n as f64 + eps as f64) as f32);
    for i in 0..n {
        out[i] = (x[i] / rms) * w[i];
    }
}

/// Apply RoPE to q or k heads
fn apply_rope(head: &mut [f32], pos: u32, hd: usize, theta: f32) {
    for i in 0..(hd / 2) {
        let inv_freq = mathf::powf(theta, -2.0 * i as f32 / hd as f32);
        let angle = (pos as f64) * (inv_freq as f64);
        let cos = mathf::cosf(angle as f32);
        let sin = mathf::sinf(angle as f32);
        let a = head[i];
        let b = head[i + hd / 2];
        head[i] = a * cos - b * sin;
        head[i + hd / 2] = a * sin + b * cos;
    }
}

/// y = W @ x (row-major [out_dim, in_dim])
#[allow(clippy::needless_range_loop)]
fn matmul_f32(w: &[f32], x: &[f32], out: &mut [f32], out_dim: usize, in_dim: usize) {
    for o in 0..out_dim {
        let mut sum = 0.0_f64;
        for i in 0..in_dim {
            sum += (w[o * in_dim + i] as f64) * (x[i] as f64);
        }
        out[o] = sum as f32;
    }
}

/// y = W @ x with Q8_0 weights
#[allow(clippy::needless_range_loop)]
fn matmul_q8(data: &[u8], x: &[f32], out: &mut [f32], out_dim: usize, in_dim: usize) {
    for o in 0..out_dim {
        let mut sum = 0.0_f64;
        for i in 0..in_dim {
            let idx = (o as u64 * in_dim as u64 + i as u64) as usize;
            let block = idx / 32;
            let lane = idx % 32;
            let block_off = block * 36;
            let qval = data[block_off + lane] as i8;
            let scale = f32::from_le_bytes([
                data[block_off + 32],
                data[block_off + 33],
                data[block_off + 34],
                data[block_off + 35],
            ]);
            sum += ((qval as f32) * scale * x[i]) as f64;
        }
        out[o] = sum as f32;
    }
}

fn softmax(logits: &[f32], out: &mut [f32]) {
    let max_l = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut sum = 0.0_f64;
    for &l in logits {
        sum += mathf::expf(l - max_l) as f64;
    }
    let norm = if sum > 0.0 { 1.0 / sum } else { 0.0 };
    for i in 0..logits.len() {
        out[i] = ((mathf::expf(logits[i] - max_l) as f64) * norm) as f32;
    }
}

pub struct Session<'a> {
    model: &'a Model<'a>,
    pos: u32,
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
}

impl<'a> Session<'a> {
    pub fn new(model: &'a Model<'a>) -> Self {
        let n_l = model.n_layers as usize;
        let csize = model.max_context as usize * model.n_kv_heads as usize * model.head_dim as usize;
        Session {
            model,
            pos: 0,
            k: (0..n_l).map(|_| vec![0.0; csize]).collect(),
            v: (0..n_l).map(|_| vec![0.0; csize]).collect(),
        }
    }

    pub fn feed(&mut self, tok: u32) -> Result<Vec<f32>, &'static str> {
        let m = self.model;
        let vs = m.vocab_size as usize;
        let d = m.dim as usize;
        let nh = m.n_heads as usize;
        let nkv = m.n_kv_heads as usize;
        let hd = m.head_dim as usize;
        let hd_f = m.hidden_dim as usize;

        if self.pos >= m.max_context {
            return Err("ctx");
        }

        let pos = self.pos;

        // Embed
        let emb_t = m.tensors.iter().find(|t| t.name == "model.embed_tokens.weight")
            .ok_or("no emb")?;
        let mut x = vec![0.0; d];
        for (i, xi) in x.iter_mut().enumerate() {
            *xi = read_f32(emb_t, tok as usize * d + i);
        }

        // Layers
        for li in 0..m.n_layers as usize {
            let prf = format!("model.layers.{}", li);

            // Input norm
            let norm_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.input_layernorm.weight", prf))
                .ok_or("no norm")?;
            let norm_w = load_tensor(norm_t, d);
            let mut h = vec![0.0; d];
            rmsnorm(&x, &norm_w, m.norm_eps, &mut h);

            // QKV
            let q_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.self_attn.q_proj.weight", prf))
                .ok_or("no q")?;
            let k_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.self_attn.k_proj.weight", prf))
                .ok_or("no k")?;
            let v_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.self_attn.v_proj.weight", prf))
                .ok_or("no v")?;

            let mut q = vec![0.0; d];
            let mut kv = vec![0.0; nkv * hd];
            let mut v = vec![0.0; nkv * hd];

            match q_t.dtype {
                DType::F32 => {
                    let w = load_tensor(q_t, d * d);
                    matmul_f32(&w, &h, &mut q, d, d);
                }
                DType::Q8_0 => matmul_q8(q_t.data, &h, &mut q, d, d),
            }

            match k_t.dtype {
                DType::F32 => {
                    let w = load_tensor(k_t, nkv * hd * d);
                    matmul_f32(&w, &h, &mut kv, nkv * hd, d);
                }
                DType::Q8_0 => matmul_q8(k_t.data, &h, &mut kv, nkv * hd, d),
            }

            match v_t.dtype {
                DType::F32 => {
                    let w = load_tensor(v_t, nkv * hd * d);
                    matmul_f32(&w, &h, &mut v, nkv * hd, d);
                }
                DType::Q8_0 => matmul_q8(v_t.data, &h, &mut v, nkv * hd, d),
            }

            // RoPE
            for i in 0..nh {
                apply_rope(&mut q[i * hd..(i + 1) * hd], pos, hd, m.rope_theta);
            }
            for i in 0..nkv {
                apply_rope(&mut kv[i * hd..(i + 1) * hd], pos, hd, m.rope_theta);
            }

            // Cache
            let ci = pos as usize * nkv * hd;
            self.k[li][ci..ci + nkv * hd].copy_from_slice(&kv);
            self.v[li][ci..ci + nkv * hd].copy_from_slice(&v);

            // Attn
            let mut attn = vec![0.0; d];
            #[allow(clippy::needless_range_loop)]
            {
                for head in 0..nh {
                    let gkv = head / (nh / nkv);
                    let qh = &q[head * hd..(head + 1) * hd];

                    let mut sc = vec![0.0; pos as usize + 1];
                    let hd_sqrt = mathf::sqrtf(hd as f32);
                    for t in 0..=pos as usize {
                        let koff = t * nkv * hd + gkv * hd;
                        let kh = &self.k[li][koff..koff + hd];
                        let mut s = 0.0_f64;
                        for i in 0..hd {
                            s += (qh[i] as f64) * (kh[i] as f64);
                        }
                        sc[t] = (s as f32) / hd_sqrt;
                    }

                    let mut pr = vec![0.0; sc.len()];
                    softmax(&sc, &mut pr);

                    let mut out = vec![0.0; hd];
                    for t in 0..=pos as usize {
                        let voff = t * nkv * hd + gkv * hd;
                        let vh = &self.v[li][voff..voff + hd];
                        for i in 0..hd {
                            out[i] += pr[t] * vh[i];
                        }
                    }

                    attn[head * hd..(head + 1) * hd].copy_from_slice(&out);
                }
            }

            // Out proj
            let o_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.self_attn.o_proj.weight", prf))
                .ok_or("no o")?;
            let mut out = vec![0.0; d];
            match o_t.dtype {
                DType::F32 => {
                    let w = load_tensor(o_t, d * d);
                    matmul_f32(&w, &attn, &mut out, d, d);
                }
                DType::Q8_0 => matmul_q8(o_t.data, &attn, &mut out, d, d),
            }

            for i in 0..d {
                x[i] += out[i];
            }

            // MLP norm
            let mn_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.post_attention_layernorm.weight", prf))
                .ok_or("no mn")?;
            let mn_w = load_tensor(mn_t, d);
            let mut h2 = vec![0.0; d];
            rmsnorm(&x, &mn_w, m.norm_eps, &mut h2);

            // Gate & up
            let g_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.mlp.gate_proj.weight", prf))
                .ok_or("no g")?;
            let u_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.mlp.up_proj.weight", prf))
                .ok_or("no u")?;

            let mut gu = vec![0.0; hd_f];
            let mut uu = vec![0.0; hd_f];

            match g_t.dtype {
                DType::F32 => {
                    let w = load_tensor(g_t, hd_f * d);
                    matmul_f32(&w, &h2, &mut gu, hd_f, d);
                }
                DType::Q8_0 => matmul_q8(g_t.data, &h2, &mut gu, hd_f, d),
            }

            match u_t.dtype {
                DType::F32 => {
                    let w = load_tensor(u_t, hd_f * d);
                    matmul_f32(&w, &h2, &mut uu, hd_f, d);
                }
                DType::Q8_0 => matmul_q8(u_t.data, &h2, &mut uu, hd_f, d),
            }

            let mut mm = vec![0.0; hd_f];
            for i in 0..hd_f {
                mm[i] = mathf::silu(gu[i]) * uu[i];
            }

            // Down
            let d_t = m.tensors.iter()
                .find(|t| t.name == format!("{}.mlp.down_proj.weight", prf))
                .ok_or("no d")?;
            let mut dout = vec![0.0; d];
            match d_t.dtype {
                DType::F32 => {
                    let w = load_tensor(d_t, d * hd_f);
                    matmul_f32(&w, &mm, &mut dout, d, hd_f);
                }
                DType::Q8_0 => matmul_q8(d_t.data, &mm, &mut dout, d, hd_f),
            }

            for i in 0..d {
                x[i] += dout[i];
            }
        }

        // Final norm
        let fn_t = m.tensors.iter()
            .find(|t| t.name == "model.norm.weight")
            .ok_or("no fn")?;
        let fn_w = load_tensor(fn_t, d);
        let mut xn = vec![0.0; d];
        rmsnorm(&x, &fn_w, m.norm_eps, &mut xn);

        // Logits
        let lm_t = m.tensors.iter()
            .find(|t| t.name == "lm_head.weight")
            .or_else(|| m.tensors.iter().find(|t| t.name == "model.embed_tokens.weight"))
            .ok_or("no lm")?;

        let mut logits = vec![0.0; vs];
        match lm_t.dtype {
            DType::F32 => {
                let w = load_tensor(lm_t, vs * d);
                matmul_f32(&w, &xn, &mut logits, vs, d);
            }
            DType::Q8_0 => matmul_q8(lm_t.data, &xn, &mut logits, vs, d),
        }

        self.pos += 1;
        Ok(logits)
    }
}
