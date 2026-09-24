//! Golden tests for the tokenizer: loads prompts and golden token IDs from the reference
//! test fixtures and verifies the tokenizer produces exactly matching output.

use std::fs;
use std::path::Path;

struct Prompt {
    id: String,
    text: String,
}

struct GoldenEntry {
    id: String,
    token_ids: Vec<u32>,
}

/// Load prompts.json (same source as reference tokenizer test).
fn load_prompts(path: &Path) -> Result<Vec<Prompt>, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;
    let value = otter_json::parse_str(&data).map_err(|e| format!("parse error: {:?}", e))?;
    let mut prompts = Vec::new();
    if let Some(arr) = value.as_array() {
        for item in arr {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
            prompts.push(Prompt { id, text });
        }
    }
    Ok(prompts)
}

/// Load golden.json (expected token IDs from reference tokenizer).
fn load_golden(path: &Path) -> Result<Vec<GoldenEntry>, String> {
    let data = fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;
    let value = otter_json::parse_str(&data).map_err(|e| format!("parse error: {:?}", e))?;
    let mut entries = Vec::new();
    if let Some(obj) = value.as_object()
        && let Some(tokenization) = obj.get("tokenization").and_then(|v| v.as_array())
    {
        for item in tokenization {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let token_ids = item
                .get("token_ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_u64().map(|u| u as u32))
                        .collect()
                })
                .unwrap_or_default();
            entries.push(GoldenEntry { id, token_ids });
        }
    }
    Ok(entries)
}

#[test]
fn tokenizer_golden_smollm2() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());

    let model_path = Path::new(&model_dir).join("SmolLM2-135M-Instruct.f32.otm");
    let prompts_path = Path::new("/Users/sammytourani/Projects/otteros/tools/otter-convert/tests/fixtures/prompts/prompts.json");
    let golden_path = Path::new("/Users/sammytourani/Projects/otteros/tools/otter-convert/tests/fixtures/smollm2/golden.json");

    if !model_path.exists() || !prompts_path.exists() || !golden_path.exists() {
        eprintln!("tokenizer_golden_smollm2: skipping (fixtures not available)");
        return;
    }

    let prompts = load_prompts(prompts_path).expect("load prompts.json");
    let golden_entries = load_golden(golden_path).expect("load golden.json");
    let model_data = fs::read(&model_path).expect("read model file");
    let model = otter_llm::loader::Model::load(&model_data).expect("load model");

    // Build tokenizer from loader data
    let tokenizer = otter_llm::Tokenizer::from_loader_data(
        model.vocab,
        model.merge_pairs,
        model.added_tokens,
        model.isolate_digits,
    ).expect("create tokenizer");

    // Build golden map for quick lookup
    let mut golden_map = std::collections::HashMap::new();
    for entry in golden_entries {
        golden_map.insert(entry.id, entry.token_ids);
    }

    let mut passed = 0;
    let mut failed = 0;

    for prompt in prompts.iter() {
        if let Some(expected_ids) = golden_map.get(&prompt.id) {
            match tokenizer.encode(&prompt.text) {
                Ok(actual_ids) => {
                    if actual_ids == *expected_ids {
                        passed += 1;
                        eprintln!("✓ SmolLM2 {}: {} tokens", prompt.id, actual_ids.len());
                    } else {
                        failed += 1;
                        let first_diff = actual_ids.iter().zip(expected_ids.iter())
                            .position(|(a, b)| a != b)
                            .unwrap_or(actual_ids.len().min(expected_ids.len()));
                        eprintln!("✗ SmolLM2 {}: mismatch at position {}", prompt.id, first_diff);
                        eprintln!("  Got {} tokens, expected {}", actual_ids.len(), expected_ids.len());
                        if first_diff < actual_ids.len().min(expected_ids.len()) + 3 {
                            let end = (first_diff + 3).min(actual_ids.len()).min(expected_ids.len());
                            eprintln!("  Got:      {:?}", &actual_ids[first_diff..end]);
                            eprintln!("  Expected: {:?}", &expected_ids[first_diff..end]);
                        }
                    }
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("✗ SmolLM2 {}: encode error: {:?}", prompt.id, e);
                }
            }
        }
    }

    eprintln!("\nSmolLM2: {}/{} passed", passed, passed + failed);
    assert_eq!(failed, 0, "SmolLM2 tokenizer golden tests");
}

#[test]
fn tokenizer_golden_tiny_random() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR")
        .unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".to_string());

    let model_path = Path::new(&model_dir).join("tiny-random.f32.otm");
    let prompts_path = Path::new("/Users/sammytourani/Projects/otteros/tools/otter-convert/tests/fixtures/prompts/prompts.json");
    let golden_path = Path::new("/Users/sammytourani/Projects/otteros/tools/otter-convert/tests/fixtures/tiny-random/golden.json");

    if !model_path.exists() || !prompts_path.exists() || !golden_path.exists() {
        eprintln!("tokenizer_golden_tiny_random: skipping (fixtures not available)");
        return;
    }

    let prompts = load_prompts(prompts_path).expect("load prompts.json");
    let golden_entries = load_golden(golden_path).expect("load golden.json");
    let model_data = fs::read(&model_path).expect("read model file");
    let model = otter_llm::loader::Model::load(&model_data).expect("load model");

    // Build tokenizer from loader data
    let tokenizer = otter_llm::Tokenizer::from_loader_data(
        model.vocab,
        model.merge_pairs,
        model.added_tokens,
        model.isolate_digits,
    ).expect("create tokenizer");

    // Build golden map for quick lookup
    let mut golden_map = std::collections::HashMap::new();
    for entry in golden_entries {
        golden_map.insert(entry.id, entry.token_ids);
    }

    let mut passed = 0;
    let mut failed = 0;

    for prompt in prompts.iter() {
        if let Some(expected_ids) = golden_map.get(&prompt.id) {
            match tokenizer.encode(&prompt.text) {
                Ok(actual_ids) => {
                    if actual_ids == *expected_ids {
                        passed += 1;
                        eprintln!("✓ tiny-random {}: {} tokens", prompt.id, actual_ids.len());
                    } else {
                        failed += 1;
                        let first_diff = actual_ids.iter().zip(expected_ids.iter())
                            .position(|(a, b)| a != b)
                            .unwrap_or(actual_ids.len().min(expected_ids.len()));
                        eprintln!("✗ tiny-random {}: mismatch at position {}", prompt.id, first_diff);
                        eprintln!("  Got {} tokens, expected {}", actual_ids.len(), expected_ids.len());
                        if first_diff < actual_ids.len().min(expected_ids.len()) + 3 {
                            let end = (first_diff + 3).min(actual_ids.len()).min(expected_ids.len());
                            eprintln!("  Got:      {:?}", &actual_ids[first_diff..end]);
                            eprintln!("  Expected: {:?}", &expected_ids[first_diff..end]);
                        }
                    }
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("✗ tiny-random {}: encode error: {:?}", prompt.id, e);
                }
            }
        }
    }

    eprintln!("\ntiny-random: {}/{} passed", passed, passed + failed);
    assert_eq!(failed, 0, "tiny-random tokenizer golden tests");
}
