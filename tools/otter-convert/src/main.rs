//! Converts Hugging Face checkpoints (Llama, Qwen2, Qwen3 architectures only,
//! DECISIONS.md D21) to OtterOS's own `.otm` format (f32 and Q8_0), or
//! generates a deterministic tiny random model for testing.
//!
//! Usage:
//!   otter-convert --model-dir <path> --out <path> --dtype {f32|q8}
//!   otter-convert --tiny-random <seed> --out <path> --dtype {f32|q8}

mod config;
mod half;
mod safetensors;
mod tokenizer;
mod otm;

use config::parse_model_config;
use safetensors::SafeTensors;
use otm::{OtmWriter, OtmDtype, quantize_q8_0};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputDtype {
    F32,
    Q8_0,
}

impl OutputDtype {
    fn from_str(s: &str) -> Result<OutputDtype, String> {
        match s {
            "f32" => Ok(OutputDtype::F32),
            "q8" => Ok(OutputDtype::Q8_0),
            other => Err(format!("unsupported output dtype \"{other}\" (f32 or q8)")),
        }
    }
}

fn convert_model(model_dir: &str, output_path: &str, dtype: OutputDtype) -> Result<(), String> {
    let model_path = PathBuf::from(model_dir);

    // Read and parse config.json
    let config_path = model_path.join("config.json");
    let config_json_str = fs::read_to_string(&config_path)
        .map_err(|e| format!("reading config.json: {e}"))?;
    let config_value = otter_json::parse_str(&config_json_str)
        .map_err(|e| format!("parsing config.json: {e}"))?;
    let config = parse_model_config(&config_value)?;

    eprintln!(
        "convert: loaded config: {:?} arch, vocab_size={}, dim={}, n_layers={}",
        config.architecture, config.vocab_size, config.dim, config.n_layers
    );

    // Read and parse tokenizer.json
    let tokenizer_path = model_path.join("tokenizer.json");
    let tokenizer_json_str = fs::read_to_string(&tokenizer_path)
        .map_err(|e| format!("reading tokenizer.json: {e}"))?;
    let tokenizer_value = otter_json::parse_str(&tokenizer_json_str)
        .map_err(|e| format!("parsing tokenizer.json: {e}"))?;
    let tokenizer = tokenizer::parse_tokenizer(&tokenizer_value)?;

    eprintln!("convert: loaded tokenizer: {} vocab entries", tokenizer.id_to_bytes.len());

    // Read safetensors
    let safetensors_path = model_path.join("model.safetensors");
    let st = SafeTensors::read(&safetensors_path)?;
    eprintln!("convert: loaded safetensors: {} tensors", st.names().len());

    // First pass: collect tensor metadata and data
    let mut tensors_to_write: Vec<(String, OtmDtype, Vec<u32>, Vec<u8>)> = Vec::new();
    for tensor_name in st.names() {
        let f32_values = st
            .to_f32(tensor_name)
            .ok_or_else(|| format!("failed to read tensor {}", tensor_name))?;
        let info = st
            .info(tensor_name)
            .ok_or_else(|| format!("tensor {} has no info", tensor_name))?;

        let tensor_data = match dtype {
            OutputDtype::F32 => {
                let mut data = Vec::with_capacity(f32_values.len() * 4);
                for v in f32_values {
                    data.extend_from_slice(&v.to_le_bytes());
                }
                data
            }
            OutputDtype::Q8_0 => quantize_q8_0(&f32_values),
        };

        let otm_dtype = match dtype {
            OutputDtype::F32 => OtmDtype::F32,
            OutputDtype::Q8_0 => OtmDtype::Q8_0,
        };

        tensors_to_write.push((tensor_name.to_string(), otm_dtype, info.shape.clone(), tensor_data));
    }

    eprintln!("convert: collected {} tensors", tensors_to_write.len());

    // Build .otm file
    let mut writer = OtmWriter::new();
    writer.write_header(&config, &tokenizer)?;
    writer.write_tokenizer(&tokenizer)?;

    // Write tensors
    for (tensor_name, otm_dtype, shape, tensor_data) in tensors_to_write {
        writer.add_tensor(&tensor_name, otm_dtype, shape, tensor_data)?;
    }

    eprintln!("convert: writing tensor table");
    writer.write_tensor_table()?;

    let data = writer.to_bytes();
    fs::write(output_path, &data).map_err(|e| format!("writing {}: {e}", output_path))?;
    eprintln!("convert: wrote {} ({} bytes)", output_path, data.len());
    Ok(())
}

/// Generate a tiny deterministic random Llama-style model for testing.
/// Writes config.json and model.safetensors that can be loaded by HuggingFace transformers.
fn generate_tiny_random(seed: u64, output_dir: &str) -> Result<(), String> {
    let dir = PathBuf::from(output_dir);
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", output_dir))?;

    // Simple LCG for deterministic randomness
    let mut rng_state = seed;
    let mut next_f32 = || {
        rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        // Extract mantissa from upper bits, create value in [1, 2), subtract 1 for [0, 1)
        let mantissa_bits = ((rng_state >> 40) & 0x7fffff) as u32;  // 23 bits of mantissa
        let bits = 0x3f800000 | mantissa_bits;  // 1.0 (0x3f800000) with random mantissa
        f32::from_bits(bits) - 1.0
    };

    // Generate random weight tensors for proper LlamaForCausalLM
    // vocab_size=49152, hidden_size=64, intermediate_size=176, num_layers=2, num_heads=4, num_kv_heads=2
    let mut tensors = Vec::new();

    // embed_tokens: (49152, 64) - shared with lm_head for tied embeddings
    let mut data = vec![0.0f32; 49152 * 64];
    for v in &mut data {
        *v = next_f32() * 0.02;
    }
    tensors.push(("model.embed_tokens.weight".to_string(), vec![49152, 64], data));

    // Two layers of transformer blocks
    for layer_idx in 0..2 {
        let prefix = format!("model.layers.{}", layer_idx);

        // self_attn.q_proj: (64, 64) = (hidden_size, hidden_size)
        let mut data = vec![0.0f32; 64 * 64];
        for v in &mut data {
            *v = next_f32() * 0.02;
        }
        tensors.push((format!("{}.self_attn.q_proj.weight", prefix), vec![64, 64], data));

        // self_attn.k_proj: (32, 64) = (num_kv_heads * head_dim, hidden_size)
        let mut data = vec![0.0f32; 32 * 64];
        for v in &mut data {
            *v = next_f32() * 0.02;
        }
        tensors.push((format!("{}.self_attn.k_proj.weight", prefix), vec![32, 64], data));

        // self_attn.v_proj: (32, 64)
        let mut data = vec![0.0f32; 32 * 64];
        for v in &mut data {
            *v = next_f32() * 0.02;
        }
        tensors.push((format!("{}.self_attn.v_proj.weight", prefix), vec![32, 64], data));

        // self_attn.o_proj: (64, 64)
        let mut data = vec![0.0f32; 64 * 64];
        for v in &mut data {
            *v = next_f32() * 0.02;
        }
        tensors.push((format!("{}.self_attn.o_proj.weight", prefix), vec![64, 64], data));

        // mlp.gate_proj: (176, 64) = (intermediate_size, hidden_size)
        let mut data = vec![0.0f32; 176 * 64];
        for v in &mut data {
            *v = next_f32() * 0.02;
        }
        tensors.push((format!("{}.mlp.gate_proj.weight", prefix), vec![176, 64], data));

        // mlp.up_proj: (176, 64)
        let mut data = vec![0.0f32; 176 * 64];
        for v in &mut data {
            *v = next_f32() * 0.02;
        }
        tensors.push((format!("{}.mlp.up_proj.weight", prefix), vec![176, 64], data));

        // mlp.down_proj: (64, 176) = (hidden_size, intermediate_size)
        let mut data = vec![0.0f32; 64 * 176];
        for v in &mut data {
            *v = next_f32() * 0.02;
        }
        tensors.push((format!("{}.mlp.down_proj.weight", prefix), vec![64, 176], data));

        // input_layernorm: (64,) = (hidden_size,)
        let data = vec![1.0f32; 64];
        tensors.push((format!("{}.input_layernorm.weight", prefix), vec![64], data));

        // post_attention_layernorm: (64,)
        let data = vec![1.0f32; 64];
        tensors.push((
            format!("{}.post_attention_layernorm.weight", prefix),
            vec![64],
            data,
        ));
    }

    // final norm: (64,) = (hidden_size,)
    let data = vec![1.0f32; 64];
    tensors.push(("model.norm.weight".to_string(), vec![64], data));

    // Note: No separate lm_head - embeddings are tied, so lm_head shares embed_tokens.weight

    safetensors::write_f32(&dir.join("model.safetensors"), &tensors)
        .map_err(|e| format!("writing safetensors: {e}"))?;

    eprintln!(
        "generate_tiny_random: wrote model.safetensors to {}",
        dir.display()
    );
    eprintln!(
        "generate_tiny_random: copy tokenizer.json and tokenizer_config.json from SmolLM2-135M-Instruct to {}",
        dir.display()
    );
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut model_dir: Option<String> = None;
    let mut out: Option<String> = None;
    let mut dtype: Option<OutputDtype> = None;
    let mut tiny_random: Option<u64> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--model-dir" => {
                if i + 1 >= args.len() {
                    eprintln!("--model-dir requires an argument");
                    std::process::exit(1);
                }
                model_dir = Some(args[i + 1].clone());
                i += 2;
            }
            "--out" => {
                if i + 1 >= args.len() {
                    eprintln!("--out requires an argument");
                    std::process::exit(1);
                }
                out = Some(args[i + 1].clone());
                i += 2;
            }
            "--dtype" => {
                if i + 1 >= args.len() {
                    eprintln!("--dtype requires an argument");
                    std::process::exit(1);
                }
                match OutputDtype::from_str(&args[i + 1]) {
                    Ok(d) => dtype = Some(d),
                    Err(e) => {
                        eprintln!("{}", e);
                        std::process::exit(1);
                    }
                }
                i += 2;
            }
            "--tiny-random" => {
                if i + 1 >= args.len() {
                    eprintln!("--tiny-random requires a seed argument");
                    std::process::exit(1);
                }
                match args[i + 1].parse::<u64>() {
                    Ok(seed) => tiny_random = Some(seed),
                    Err(e) => {
                        eprintln!("--tiny-random seed must be a number: {}", e);
                        std::process::exit(1);
                    }
                }
                i += 2;
            }
            other => {
                eprintln!("unknown argument: {}", other);
                std::process::exit(1);
            }
        }
    }

    let out = out.expect("--out is required");
    let dtype = dtype.expect("--dtype is required");

    if let Some(seed) = tiny_random {
        if let Err(e) = generate_tiny_random(seed, &out) {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    } else {
        let model_dir = model_dir.expect("--model-dir is required (unless using --tiny-random)");
        if let Err(e) = convert_model(&model_dir, &out, dtype) {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}
