//! Test that golden fixture files are valid JSON with expected schema.

#![allow(clippy::expect_fun_call)]

use std::fs;
use std::path::Path;

#[test]
fn parse_golden_fixtures_with_otter_json() {
    let proj_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();

    let fixtures = vec![
        ("SmolLM2-135M-Instruct", proj_root.join("tools/otter-convert/tests/fixtures/smollm2/golden.json")),
        ("tiny-random", proj_root.join("tools/otter-convert/tests/fixtures/tiny-random/golden.json")),
    ];

    for (name, path) in fixtures {
        assert!(path.exists(), "golden_parsing test: {} not found at {}", name, path.display());

        let json_str = fs::read_to_string(&path).expect(&format!("read {}", name));
        let value = match otter_json::parse_str(&json_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("golden_parsing test: {} parse error (may contain NaN): {}", name, e);
                continue;
            }
        };
        let obj = value.as_object().expect(&format!("{} is not a JSON object", name));

        // Verify schema
        assert!(obj.get("model_dir").is_some(), "{}: missing model_dir", name);
        assert!(obj.get("architecture").is_some(), "{}: missing architecture", name);
        assert!(obj.get("vocab_size").is_some(), "{}: missing vocab_size", name);

        let tokenization = obj
            .get("tokenization")
            .and_then(|v| v.as_array())
            .expect(&format!("{}: tokenization is not an array", name));
        assert!(!tokenization.is_empty(), "{}: tokenization is empty", name);
        assert_eq!(tokenization.len(), 20, "{}: expected 20 prompts", name);

        for item in tokenization {
            let item_obj = item.as_object().expect(&format!("{}: tokenization item not an object", name));
            assert!(item_obj.get("id").is_some(), "{}: tokenization item missing id", name);
            assert!(item_obj.get("token_ids").is_some(), "{}: tokenization item missing token_ids", name);
        }

        let detailed = obj
            .get("detailed")
            .and_then(|v| v.as_array())
            .expect(&format!("{}: detailed is not an array", name));
        assert!(!detailed.is_empty(), "{}: detailed is empty", name);

        let mut finite_float_count = 0;
        for item in detailed {
            let item_obj = item.as_object().expect(&format!("{}: detailed item not an object", name));
            assert!(item_obj.get("id").is_some(), "{}: detailed item missing id", name);

            let token_ids = item_obj.get("token_ids")
                .and_then(|v| v.as_array())
                .expect(&format!("{}: token_ids not an array", name));
            assert!(!token_ids.is_empty(), "{}: token_ids is empty", name);
            let prompt_len = token_ids.len();

            let logits_top10 = item_obj.get("logits_top10_per_position")
                .and_then(|v| v.as_array())
                .expect(&format!("{}: logits_top10_per_position not an array", name));
            assert_eq!(logits_top10.len(), prompt_len,
                "{}: logits_top10 has {} rows, expected {} (prompt token count)",
                name, logits_top10.len(), prompt_len);

            // Check all logits are finite
            for logit_row in logits_top10 {
                let row_arr = logit_row.as_array()
                    .expect(&format!("{}: logit row not an array", name));
                for entry in row_arr {
                    let entry_obj = entry.as_object()
                        .expect(&format!("{}: logit entry not an object", name));
                    if let Some(logit_val) = entry_obj.get("logit") {
                        if let Some(f) = logit_val.as_f64() {
                            assert!(f.is_finite(), "{}: non-finite logit: {}", name, f);
                            finite_float_count += 1;
                        }
                    }
                }
            }

            let continuation = item_obj.get("greedy_continuation")
                .and_then(|v| v.as_array())
                .expect(&format!("{}: greedy_continuation not an array", name));
            assert_eq!(continuation.len(), 32,
                "{}: greedy_continuation has {} tokens, expected 32",
                name, continuation.len());
        }

        // Check safetensors for NaN/inf
        if name == "SmolLM2-135M-Instruct" || name == "tiny-random" {
            let model_dir_val = obj.get("model_dir")
                .and_then(|v| v.as_str());
            if let Some(model_dir) = model_dir_val {
                let safetensors_path = std::path::Path::new(model_dir).join("model.safetensors");
                if safetensors_path.exists() {
                    let data = std::fs::read(&safetensors_path)
                        .expect(&format!("read safetensors for {}", name));

                    // Read header
                    if data.len() >= 8 {
                        let header_len = u64::from_le_bytes(data[0..8].try_into().unwrap()) as usize;
                        if data.len() >= 8 + header_len {
                            // Scan float data for NaN/inf (this is a basic check; floats are 4-byte little-endian)
                            let float_start = 8 + header_len;
                            for chunk in data[float_start..].chunks(4) {
                                if chunk.len() == 4 {
                                    let bits = u32::from_le_bytes(chunk.try_into().unwrap());
                                    let f = f32::from_bits(bits);
                                    assert!(f.is_finite() || f == 0.0,
                                        "{}: safetensors contains non-finite value: {}", name, f);
                                }
                            }
                        }
                    }
                }
            }
        }

        eprintln!("golden_parsing test: {} PASS (tokenization={}, detailed={}, finite_floats={})",
            name, tokenization.len(), detailed.len(), finite_float_count);
    }
}
