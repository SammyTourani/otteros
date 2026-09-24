//! Tests that validate the tokenizer reproduces reference token ids from HuggingFace.
//! Brief M7-T1: "tokenizer section reproduces the reference token ids for all 20 prompts"

use std::fs;
use std::path::Path;

#[test]
fn tokenizer_reproduces_golden_token_ids_for_smollm2() {
    let proj_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let prompts_path = proj_root.join("tools/otter-convert/tests/fixtures/prompts/prompts.json");
    let golden_path = proj_root.join("tools/otter-convert/tests/fixtures/smollm2/golden.json");

    // Try to find the model in multiple locations
    let model_dir = {
        let candidate1 = proj_root.join("third_party/HuggingFaceTB/SmolLM2-135M-Instruct");
        if candidate1.is_dir() {
            candidate1
        } else {
            // Try OTTEROS_MODEL_DIR environment variable
            match std::env::var("OTTEROS_MODEL_DIR") {
                Ok(base) => {
                    std::path::PathBuf::from(base).join("HuggingFaceTB/SmolLM2-135M-Instruct")
                }
                Err(_) => candidate1, // Use the default path even if it doesn't exist (will skip below)
            }
        }
    };

    // If fixtures are missing, skip gracefully
    if !model_dir.is_dir() || !prompts_path.exists() || !golden_path.exists() {
        eprintln!("tokenizer test: skipping (fixtures not complete; run scripts/fetch-models.sh and scripts/llm-reference.py)");
        return;
    }

    // Parse prompts.json
    let prompts_json_str = fs::read_to_string(&prompts_path).expect("read prompts.json");
    let prompts_value = otter_json::parse_str(&prompts_json_str).expect("parse prompts.json");
    let prompts_array = prompts_value
        .as_array()
        .expect("prompts.json is not an array");

    // Parse golden.json
    let golden_json_str = fs::read_to_string(&golden_path).expect("read golden.json");
    let golden_value = otter_json::parse_str(&golden_json_str).expect("parse golden.json");
    let golden_obj = golden_value.as_object().expect("golden.json is not an object");
    let golden_tokenization = golden_obj
        .get("tokenization")
        .and_then(|v| v.as_array())
        .expect("golden.json missing tokenization array");

    // Build a map of prompt_id -> expected token ids from golden
    let mut golden_ids_map = std::collections::HashMap::new();
    for item in golden_tokenization {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .expect("tokenization item missing id");
        let ids: Vec<u32> = item
            .get("token_ids")
            .and_then(|v| v.as_array())
            .expect("tokenization item missing token_ids")
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect();
        golden_ids_map.insert(id.to_string(), ids);
    }

    // Parse the tokenizer
    let tokenizer_path = model_dir.join("tokenizer.json");
    let tokenizer_json_str = fs::read_to_string(&tokenizer_path).expect("read tokenizer.json");
    let tokenizer_value = otter_json::parse_str(&tokenizer_json_str).expect("parse tokenizer.json");
    let tokenizer =
        otter_convert::tokenizer::parse_tokenizer(&tokenizer_value).expect("parse tokenizer");

    // Test each prompt
    let mut passed = 0;
    let mut failed = 0;
    let mut failures = Vec::new();

    for prompt_obj in prompts_array {
        let prompt_id = prompt_obj
            .get("id")
            .and_then(|v| v.as_str())
            .expect("prompt missing id");
        let prompt_text = prompt_obj
            .get("text")
            .and_then(|v| v.as_str())
            .expect("prompt missing text");

        if let Some(expected_ids) = golden_ids_map.get(prompt_id) {
            match otter_convert::tokenizer::encode(&tokenizer, prompt_text) {
                Ok(actual_ids) => {
                    if actual_ids == *expected_ids {
                        passed += 1;
                        eprintln!("tokenizer test: {} PASS ({} tokens)", prompt_id, actual_ids.len());
                    } else {
                        failed += 1;
                        eprintln!(
                            "tokenizer test: {} FAIL: got {} tokens, expected {}",
                            prompt_id,
                            actual_ids.len(),
                            expected_ids.len()
                        );
                        failures.push(format!(
                            "{}: got {:?}, expected {:?}",
                            prompt_id, actual_ids, expected_ids
                        ));
                    }
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("tokenizer test: {} ERROR: {}", prompt_id, e);
                    failures.push(format!("{}: {}", prompt_id, e));
                }
            }
        } else {
            eprintln!("tokenizer test: {} SKIP (not in golden.json)", prompt_id);
        }
    }

    eprintln!(
        "tokenizer test: checked {} prompts, {} passed, {} failed",
        passed + failed,
        passed,
        failed
    );
    assert!(failures.is_empty(), "tokenizer failures:\n{}", failures.join("\n"));
}
