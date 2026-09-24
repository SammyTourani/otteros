//! .otm file writer: produces OtterOS model format from config + safetensors.

use crate::config::ModelConfig;
use crate::tokenizer::Tokenizer;

const OTM_MAGIC: u32 = 0x004D_544F; // "OTM\0"
const OTM_VERSION: u32 = 1;
const HEADER_SIZE: usize = 128;
const ALIGN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtmDtype {
    F32 = 0,
    Q8_0 = 1,
}

#[derive(Default)]
pub struct OtmWriter {
    data: Vec<u8>,
    tensor_offsets: Vec<(String, OtmDtype, Vec<u32>, u64, u64)>,
}

impl OtmWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_header(&mut self, cfg: &ModelConfig, _tokenizer: &Tokenizer) -> Result<(), String> {
        // Magic + version
        self.data.extend_from_slice(&OTM_MAGIC.to_le_bytes());
        self.data.extend_from_slice(&OTM_VERSION.to_le_bytes());

        // Model config
        self.data.extend_from_slice(&cfg.vocab_size.to_le_bytes());
        self.data.extend_from_slice(&cfg.dim.to_le_bytes());
        self.data.extend_from_slice(&cfg.n_layers.to_le_bytes());
        self.data.extend_from_slice(&cfg.n_heads.to_le_bytes());
        self.data.extend_from_slice(&cfg.n_kv_heads.to_le_bytes());
        self.data.extend_from_slice(&cfg.head_dim.to_le_bytes());
        self.data.extend_from_slice(&cfg.hidden_dim.to_le_bytes());
        self.data.extend_from_slice(&cfg.rope_theta.to_le_bytes());
        self.data.extend_from_slice(&cfg.norm_eps.to_le_bytes());
        self.data.extend_from_slice(&cfg.max_context.to_le_bytes());

        // Flags
        self.data.extend_from_slice(&(if cfg.tied_embeddings { 1u32 } else { 0 }).to_le_bytes());
        self.data.extend_from_slice(&(if cfg.qkv_bias { 1u32 } else { 0 }).to_le_bytes());
        self.data.extend_from_slice(&(if cfg.qk_norm { 1u32 } else { 0 }).to_le_bytes());

        // Tokenizer metadata
        self.data
            .extend_from_slice(&cfg.bos_id.unwrap_or(u32::MAX).to_le_bytes());
        self.data
            .extend_from_slice(&cfg.eos_id.unwrap_or(u32::MAX).to_le_bytes());
        self.data.extend_from_slice(&0u32.to_le_bytes()); // isolate_digits

        // Chat template
        self.data.extend_from_slice(&0u32.to_le_bytes()); // chat_template_kind = ChatML

        // Pad to header size
        while self.data.len() < HEADER_SIZE {
            self.data.push(0);
        }

        Ok(())
    }

    pub fn write_tokenizer(&mut self, tokenizer: &Tokenizer) -> Result<(), String> {
        // Vocab entries
        let vocab_len = tokenizer.id_to_bytes.len() as u32;
        self.data.extend_from_slice(&vocab_len.to_le_bytes());

        for bytes in &tokenizer.id_to_bytes {
            let len = bytes.len() as u16;
            self.data.extend_from_slice(&len.to_le_bytes());
            self.data.extend_from_slice(bytes);
        }

        // Merges: write in priority order (by rank)
        let mut merges_by_rank: Vec<_> = tokenizer.merge_rank
            .iter()
            .map(|((l, r), rank)| (*rank, l.clone(), r.clone()))
            .collect();
        merges_by_rank.sort_by_key(|t| t.0);

        self.data.extend_from_slice(&(merges_by_rank.len() as u32).to_le_bytes());
        for (_, left_str, right_str) in merges_by_rank {
            let left_id = tokenizer.vocab.get(&left_str)
                .ok_or_else(|| format!("merge left token '{}' not in vocab", left_str))?;
            let right_id = tokenizer.vocab.get(&right_str)
                .ok_or_else(|| format!("merge right token '{}' not in vocab", right_str))?;
            self.data.extend_from_slice(&left_id.to_le_bytes());
            self.data.extend_from_slice(&right_id.to_le_bytes());
        }

        // Added tokens
        self.data
            .extend_from_slice(&(tokenizer.added_tokens.len() as u32).to_le_bytes());
        for added_token in &tokenizer.added_tokens {
            self.data.extend_from_slice(&added_token.id.to_le_bytes());
            let content_bytes = added_token.content.as_bytes();
            self.data
                .extend_from_slice(&(content_bytes.len() as u16).to_le_bytes());
            self.data.extend_from_slice(content_bytes);
        }

        Ok(())
    }

    pub fn add_tensor(
        &mut self,
        name: &str,
        dtype: OtmDtype,
        shape: Vec<u32>,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let num_elements = shape.iter().map(|&d| d as u64).product::<u64>();
        let offset = self.data.len() as u64;

        // Align to 64 bytes
        let padding = (ALIGN - (offset as usize % ALIGN)) % ALIGN;
        self.data.resize(self.data.len() + padding, 0);
        let aligned_offset = self.data.len() as u64;

        self.data.extend_from_slice(&data);

        self.tensor_offsets
            .push((name.to_string(), dtype, shape, aligned_offset, num_elements));

        Ok(())
    }

    pub fn write_tensor_table(&mut self) -> Result<(), String> {
        // Record offset where tensor table starts
        let table_offset = self.data.len() as u64;

        let num_tensors = self.tensor_offsets.len() as u32;
        eprintln!("[otm] write_tensor_table: {} tensor entries at offset {}", num_tensors, table_offset);
        self.data.extend_from_slice(&num_tensors.to_le_bytes());

        let mut written = 0;
        for (name, dtype, shape, offset, num_elements) in &self.tensor_offsets {
            let name_bytes = name.as_bytes();
            self.data.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            self.data.extend_from_slice(name_bytes);

            self.data.extend_from_slice(&(*dtype as u32).to_le_bytes());
            self.data.extend_from_slice(&(shape.len() as u32).to_le_bytes());
            for &dim in shape {
                self.data.extend_from_slice(&dim.to_le_bytes());
            }
            self.data.extend_from_slice(&offset.to_le_bytes());
            self.data.extend_from_slice(&num_elements.to_le_bytes());
            written += 1;
        }
        eprintln!("[otm] write_tensor_table: finished writing {} tensor entries", written);

        // Write footer with tensor table offset for easy reading
        self.data.extend_from_slice(&table_offset.to_le_bytes());

        Ok(())
    }

    #[allow(clippy::wrong_self_convention)]
    pub fn to_bytes(self) -> Vec<u8> {
        self.data
    }
}

/// Quantize f32 values to Q8_0 format: blocks of 32 int8 + one f32 scale.
pub fn quantize_q8_0(values: &[f32]) -> Vec<u8> {
    let mut result = Vec::new();

    for chunk in values.chunks(32) {
        let max_abs = chunk.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let scale = if max_abs > 0.0 {
            max_abs / 127.0
        } else {
            1.0
        };

        for &v in chunk {
            let quantized = (v / scale).round() as i8;
            result.push(quantized as u8);
        }

        // Pad to 32 values if needed
        result.resize(result.len() + (32 - chunk.len()), 0);

        result.extend_from_slice(&scale.to_le_bytes());
    }

    result
}

/// .otm file reader: parses and validates the file structure.
#[allow(dead_code)]
pub struct OtmReader {
    data: Vec<u8>,
}

impl OtmReader {
    #[allow(dead_code)]
    pub fn read(path: &std::path::Path) -> Result<OtmReader, String> {
        let data = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        Ok(OtmReader { data })
    }

    #[allow(dead_code)]
    pub fn parse(&self) -> Result<ParsedOtm, String> {
        if self.data.len() < HEADER_SIZE + 8 {
            return Err(format!("file too small: {} bytes", self.data.len()));
        }

        // Read footer with tensor table offset
        let footer_offset = self.data.len() - 8;
        let table_offset = u64::from_le_bytes(self.data[footer_offset..footer_offset + 8].try_into().unwrap()) as usize;
        eprintln!("[otm] parse: tensor table at offset {}", table_offset);

        if table_offset >= footer_offset {
            return Err(format!("invalid tensor table offset: {}", table_offset));
        }

        // Read header
        let mut offset = 0usize;
        let magic = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let version = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;

        if magic != OTM_MAGIC {
            return Err(format!("invalid magic: 0x{:x}", magic));
        }
        if version != OTM_VERSION {
            return Err(format!("unsupported version: {}", version));
        }

        // Read model config
        let vocab_size = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let dim = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let n_layers = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let n_heads = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let n_kv_heads = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let head_dim = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let hidden_dim = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let rope_theta = f32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let norm_eps = f32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let max_context = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;

        let tied_embeddings = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) != 0;
        offset += 4;
        let qkv_bias = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) != 0;
        offset += 4;
        let qk_norm = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) != 0;
        offset += 4;

        let bos_id = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let eos_id = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
        offset += 4;
        let isolate_digits = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) != 0;
        offset += 4;
        let chat_template_kind = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());

        // Skip padding to header end
        offset = HEADER_SIZE;

        // Read tokenizer section
        let vocab_len = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;

        for _ in 0..vocab_len {
            if offset + 2 > self.data.len() {
                return Err("truncated vocab entry".to_string());
            }
            let len = u16::from_le_bytes(self.data[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2 + len;
        }

        if offset + 4 > self.data.len() {
            return Err("truncated merges count".to_string());
        }
        let merges_len = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4 + merges_len * 8; // 2 u32 per merge

        if offset + 4 > self.data.len() {
            return Err("truncated added_tokens count".to_string());
        }
        let added_tokens_len = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;

        for _ in 0..added_tokens_len {
            if offset + 6 > self.data.len() {
                return Err("truncated added token".to_string());
            }
            offset += 4; // id
            let len = u16::from_le_bytes(self.data[offset..offset + 2].try_into().unwrap()) as usize;
            offset += 2 + len;
        }

        // Read tensor table from the recorded offset
        offset = table_offset;
        if offset + 4 > self.data.len() {
            return Err("truncated tensor count".to_string());
        }
        let num_tensors = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        eprintln!("[otm] parse: reading {} tensors from offset {}", num_tensors, table_offset);

        let mut tensors = Vec::new();
        for _ in 0..num_tensors {
            if offset + 4 > self.data.len() {
                return Err("truncated tensor name length".to_string());
            }
            let name_len = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            let name = String::from_utf8_lossy(&self.data[offset..offset + name_len]).to_string();
            offset += name_len;

            if offset + 4 > self.data.len() {
                return Err("truncated dtype field".to_string());
            }
            let dtype = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            if offset + 4 > self.data.len() {
                return Err("truncated num_dims field".to_string());
            }
            let num_dims = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            if offset + (num_dims * 4) > self.data.len() {
                return Err("truncated shape fields".to_string());
            }
            let mut shape = Vec::new();
            for _ in 0..num_dims {
                let dim = u32::from_le_bytes(self.data[offset..offset + 4].try_into().unwrap());
                shape.push(dim);
                offset += 4;
            }

            if offset + 16 > self.data.len() {
                return Err("truncated tensor offset/num_elements fields".to_string());
            }
            let tensor_offset = u64::from_le_bytes(self.data[offset..offset + 8].try_into().unwrap());
            offset += 8;
            let num_elements = u64::from_le_bytes(self.data[offset..offset + 8].try_into().unwrap());
            offset += 8;

            // Verify offset is 64-byte aligned
            if tensor_offset % 64 != 0 {
                return Err(format!("tensor {} offset {} not 64-byte aligned", name, tensor_offset));
            }

            tensors.push((name, dtype, shape, tensor_offset, num_elements));
        }

        Ok(ParsedOtm {
            vocab_size,
            dim,
            n_layers,
            n_heads,
            n_kv_heads,
            head_dim,
            hidden_dim,
            rope_theta,
            norm_eps,
            max_context,
            tied_embeddings,
            qkv_bias,
            qk_norm,
            bos_id: if bos_id == u32::MAX { None } else { Some(bos_id) },
            eos_id: if eos_id == u32::MAX { None } else { Some(eos_id) },
            isolate_digits,
            chat_template_kind,
            tensors,
        })
    }
}

#[allow(dead_code)]
pub struct ParsedOtm {
    pub vocab_size: u32,
    pub dim: u32,
    pub n_layers: u32,
    pub n_heads: u32,
    pub n_kv_heads: u32,
    pub head_dim: u32,
    pub hidden_dim: u32,
    pub rope_theta: f32,
    pub norm_eps: f32,
    pub max_context: u32,
    pub tied_embeddings: bool,
    pub qkv_bias: bool,
    pub qk_norm: bool,
    pub bos_id: Option<u32>,
    pub eos_id: Option<u32>,
    pub isolate_digits: bool,
    pub chat_template_kind: u32,
    pub tensors: Vec<(String, u32, Vec<u32>, u64, u64)>,
}

/// Dequantize Q8_0 blocks: return f32 values from quantized bytes.
#[allow(dead_code)]
pub fn dequantize_q8_0(data: &[u8]) -> Result<Vec<f32>, String> {
    if !data.len().is_multiple_of(36) {
        return Err(format!("Q8_0 data length not multiple of 36: {}", data.len()));
    }

    let mut result = Vec::new();
    let (chunks, _remainder) = data.as_chunks::<36>();
    for chunk in chunks {
        let q_bytes = &chunk[..32];
        let scale_bytes: [u8; 4] = chunk[32..36].try_into().unwrap();
        let scale = f32::from_le_bytes(scale_bytes);

        for &qb in q_bytes {
            let q = qb as i8 as f32;
            result.push(q * scale);
        }
    }

    Ok(result)
}
