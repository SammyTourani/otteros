//! Tests for the .otm file loader.
//! Tests that tiny-random and SmolLM2 files load correctly with expected config.
//! Tests robustness: truncation and corruption never panic.

use std::path::Path;

#[test]
fn loader_tiny_random_f32() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let path = Path::new(&model_dir).join("tiny-random.f32.otm");

    if !path.exists() {
        eprintln!("loader test: skipping tiny-random.f32 (not found)");
        return;
    }

    let data = std::fs::read(&path).expect("read file");
    let model = otter_llm::loader::Model::load(&data).expect("load model");

    assert_eq!(model.vocab_size, 49152, "tiny-random vocab_size");
    assert_eq!(model.dim, 64, "tiny-random dim");
    assert_eq!(model.n_layers, 2, "tiny-random n_layers");
    assert_eq!(model.n_heads, 4, "tiny-random n_heads");
    assert_eq!(model.vocab.len(), 49152, "tiny-random vocab entries");

    eprintln!(
        "loader test: tiny-random.f32 PASS (vocab={}, dim={}, n_layers={}, tensors={})",
        model.vocab_size,
        model.dim,
        model.n_layers,
        model.tensors.len()
    );
}

#[test]
fn loader_tiny_random_q8() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let path = Path::new(&model_dir).join("tiny-random.q8.otm");

    if !path.exists() {
        eprintln!("loader test: skipping tiny-random.q8 (not found)");
        return;
    }

    let data = std::fs::read(&path).expect("read file");
    let model = otter_llm::loader::Model::load(&data).expect("load model");

    assert_eq!(model.vocab_size, 49152);
    assert_eq!(model.dim, 64);
    eprintln!(
        "loader test: tiny-random.q8 PASS (tensors={})",
        model.tensors.len()
    );
}

#[test]
fn loader_smollm2_f32() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let path = Path::new(&model_dir).join("SmolLM2-135M-Instruct.f32.otm");

    if !path.exists() {
        eprintln!("loader test: skipping SmolLM2.f32 (not found)");
        return;
    }

    let data = std::fs::read(&path).expect("read file");
    let model = otter_llm::loader::Model::load(&data).expect("load model");

    assert_eq!(model.vocab_size, 49152, "SmolLM2 vocab_size");
    assert_eq!(model.tensors.len(), 272, "SmolLM2 tensor count");

    eprintln!(
        "loader test: SmolLM2.f32 PASS (vocab={}, dim={}, n_layers={}, tensors={})",
        model.vocab_size,
        model.dim,
        model.n_layers,
        model.tensors.len()
    );
}

#[test]
fn loader_smollm2_q8() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let path = Path::new(&model_dir).join("SmolLM2-135M-Instruct.q8.otm");

    if !path.exists() {
        eprintln!("loader test: skipping SmolLM2.q8 (not found)");
        return;
    }

    let data = std::fs::read(&path).expect("read file");
    let model = otter_llm::loader::Model::load(&data).expect("load model");

    assert_eq!(model.vocab_size, 49152);
    assert_eq!(model.tensors.len(), 272);
    eprintln!(
        "loader test: SmolLM2.q8 PASS (tensors={})",
        model.tensors.len()
    );
}

#[test]
fn loader_robustness_truncation() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let path = Path::new(&model_dir).join("tiny-random.f32.otm");

    if !path.exists() {
        eprintln!("loader test: skipping robustness (file not found)");
        return;
    }

    let data = std::fs::read(&path).expect("read file");
    let len = data.len();

    // Test 300 evenly spaced truncation points
    for i in 0..300 {
        let trunc_len = (i * len) / 300;
        let truncated = &data[..trunc_len.min(len)];

        // This should never panic
        let _ = otter_llm::loader::Model::load(truncated);
    }

    eprintln!(
        "loader test: truncation robustness PASS (tested {} offsets, 0 panics)",
        300
    );
}

#[test]
fn loader_robustness_corruption() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let path = Path::new(&model_dir).join("tiny-random.f32.otm");

    if !path.exists() {
        eprintln!("loader test: skipping corruption (file not found)");
        return;
    }

    let mut data = std::fs::read(&path).expect("read file");
    let len = data.len();

    // Flip 300 random bytes
    use std::collections::HashSet;
    let mut flipped_positions = HashSet::new();

    for _ in 0..300 {
        let idx = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as usize) % len;

        if !flipped_positions.contains(&idx) {
            flipped_positions.insert(idx);
            data[idx] ^= 0xFF; // Flip all bits
        }
    }

    // This should never panic
    let _ = otter_llm::loader::Model::load(&data);

    eprintln!(
        "loader test: corruption robustness PASS ({} bytes corrupted, no panic)",
        flipped_positions.len()
    );
}

#[test]
fn loader_tensor_validation() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());
    let path = Path::new(&model_dir).join("tiny-random.f32.otm");

    if !path.exists() {
        eprintln!("loader test: skipping tensor validation (file not found)");
        return;
    }

    let data = std::fs::read(&path).expect("read file");
    let model = otter_llm::loader::Model::load(&data).expect("load model");

    // Verify all tensor offsets are 64-byte aligned
    for tensor in model.tensors {
        assert_eq!(
            tensor.offset % 64,
            0,
            "tensor {} offset {} not 64-byte aligned",
            tensor.name,
            tensor.offset
        );
        assert!(!tensor.shape.is_empty(), "tensor {} has empty shape", tensor.name);
        assert!(!tensor.data.is_empty(), "tensor {} has no data", tensor.name);
    }

    eprintln!(
        "loader test: tensor validation PASS (all {} tensors aligned)",
        model.tensors.len()
    );
}
