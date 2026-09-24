//! Reads a Hugging Face `config.json` into the normalized fields OtterOS's
//! `.otm` format needs (brief M7-T1, DECISIONS.md D21: "Llama-family
//! architectures only").

use otter_json::Value;

/// The three `.otm`-eligible architectures. Anything else is rejected with a
/// clear message rather than silently guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    Llama,
    Qwen2,
    Qwen3,
}

impl Architecture {
    fn from_hf_name(name: &str) -> Option<Architecture> {
        match name {
            "LlamaForCausalLM" => Some(Architecture::Llama),
            "Qwen2ForCausalLM" => Some(Architecture::Qwen2),
            "Qwen3ForCausalLM" => Some(Architecture::Qwen3),
            _ => None,
        }
    }
}

/// The subset of a Hugging Face model config that `.otm`'s header needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelConfig {
    pub architecture: Architecture,
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
    /// Whether the q/k/v projections carry a bias term.
    pub qkv_bias: bool,
    /// Whether Q and K get an extra per-head RMSNorm (Qwen3).
    pub qk_norm: bool,
    /// `bos_token_id`/`eos_token_id`, when present in `config.json` (the
    /// tokenizer section can also supply these; `config.json`'s copy wins
    /// when both exist, since it's what the reference model actually uses
    /// at generation time).
    pub bos_id: Option<u32>,
    pub eos_id: Option<u32>,
}

fn get_u32(cfg: &Value, key: &str) -> Option<u32> {
    cfg.get(key).and_then(|v| v.as_u64()).map(|v| v as u32)
}

fn require_u32(cfg: &Value, key: &str) -> Result<u32, String> {
    get_u32(cfg, key).ok_or_else(|| format!("config.json missing required integer field \"{key}\""))
}

fn get_f32(cfg: &Value, key: &str) -> Option<f32> {
    cfg.get(key).and_then(|v| v.as_f64()).map(|v| v as f32)
}

/// Parses and validates `config.json`'s contents (already-parsed JSON).
/// Returns a clear, human-readable error for an unsupported architecture or
/// a missing required field -- this is the "clear message" the brief asks
/// for when rejecting anything that isn't Llama/Qwen2/Qwen3.
pub fn parse_model_config(cfg: &Value) -> Result<ModelConfig, String> {
    let architectures = cfg
        .get("architectures")
        .and_then(Value::as_array)
        .ok_or_else(|| "config.json missing \"architectures\" array".to_string())?;
    let arch_name = architectures
        .first()
        .and_then(Value::as_str)
        .ok_or_else(|| "config.json's \"architectures\" array is empty or not strings".to_string())?;
    let architecture = Architecture::from_hf_name(arch_name).ok_or_else(|| {
        format!(
            "unsupported architecture \"{arch_name}\": otter-convert only supports \
             LlamaForCausalLM, Qwen2ForCausalLM and Qwen3ForCausalLM (DECISIONS.md D21)"
        )
    })?;

    let dim = require_u32(cfg, "hidden_size")?;
    let n_heads = require_u32(cfg, "num_attention_heads")?;
    // Older Llama configs have no GQA and omit this field entirely: it
    // defaults to `n_heads` (every head is its own KV head).
    let n_kv_heads = get_u32(cfg, "num_key_value_heads").unwrap_or(n_heads);
    // Not every config states `head_dim` explicitly (Qwen3 does; Llama
    // usually doesn't); when absent it's always `dim / n_heads`.
    let head_dim = get_u32(cfg, "head_dim").unwrap_or_else(|| dim / n_heads.max(1));

    // `attention_bias` is how Llama and Qwen3 spell "no qkv bias", but
    // Qwen2's original architecture always adds a bias to q/k/v regardless
    // of any config flag -- some published Qwen2 configs make this explicit
    // with `"attention_bias": true`, others rely on the architectural
    // default. An explicit field always wins; absent, only Qwen2 defaults
    // to `true`. (Not exercised by this brief's own fixtures, which are
    // Llama-only; flagged in the report as unverified against a real Qwen
    // checkpoint.)
    let qkv_bias = get_u32(cfg, "attention_bias")
        .map(|v| v != 0)
        .or_else(|| cfg.get("attention_bias").and_then(Value::as_bool))
        .unwrap_or(architecture == Architecture::Qwen2);

    // Qwen3's per-head Q/K RMSNorm is architectural, not config-gated in the
    // published configs seen so far.
    let qk_norm = architecture == Architecture::Qwen3;

    Ok(ModelConfig {
        architecture,
        vocab_size: require_u32(cfg, "vocab_size")?,
        dim,
        n_layers: require_u32(cfg, "num_hidden_layers")?,
        n_heads,
        n_kv_heads,
        head_dim,
        hidden_dim: require_u32(cfg, "intermediate_size")?,
        rope_theta: get_f32(cfg, "rope_theta").unwrap_or(10000.0),
        norm_eps: get_f32(cfg, "rms_norm_eps").unwrap_or(1e-5),
        max_context: require_u32(cfg, "max_position_embeddings")?,
        tied_embeddings: cfg.get("tie_word_embeddings").and_then(Value::as_bool).unwrap_or(false),
        qkv_bias,
        qk_norm,
        bos_id: get_u32(cfg, "bos_token_id"),
        eos_id: get_u32(cfg, "eos_token_id"),
    })
}
