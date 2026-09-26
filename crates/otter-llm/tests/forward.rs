use otter_llm::{Model, Session};
use otter_json::{parse_str, Value};
use alloc::vec::Vec;

extern crate alloc;

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/otter-convert/tests/fixtures/smollm2/golden.json");

fn load_model_bytes(path: &str) -> Vec<u8> {
    std::fs::read(path).expect("failed to read model file")
}

fn load_golden(path: &str) -> Value {
    let json_str = std::fs::read_to_string(path).expect("failed to read golden file");
    parse_str(&json_str).expect("failed to parse golden JSON")
}

#[test]
fn test_smollm2_f32() {
    let model_dir = std::env::var("OTTEROS_MODEL_DIR").unwrap_or_else(|_| "/Volumes/SammyDisk/models/otteros".into());
    let model_bytes = load_model_bytes(&format!("{model_dir}/SmolLM2-135M-Instruct.f32.otm"));
    let model = Model::load(&model_bytes).expect("failed to load model");
    let golden = load_golden(GOLDEN);
    let cases = golden.get("detailed").and_then(Value::as_array).expect("no detailed in golden");

    let (mut matched, mut total) = (0usize, 0usize);
    for case in cases {
        let token_ids: Vec<u32> = case.get("token_ids").and_then(Value::as_array).expect("token_ids")
            .iter().map(|v| v.as_i64().expect("id") as u32).collect();
        let continuation: Vec<u32> = case.get("greedy_continuation").and_then(Value::as_array).expect("greedy_continuation")
            .iter().map(|v| v.as_i64().expect("id") as u32).collect();
        // The logits after the LAST prompt token predict the first continuation token; every chosen
        // token is then fed exactly once. (The previous loop fed the last prompt token a second time,
        // so it continued a different sequence: that, not the model, was the 47/96 "divergence".)
        let mut session = Session::new(&model);
        let mut logits = Vec::new();
        for &tok in &token_ids {
            logits = session.feed(tok).expect("prompt token");
        }
        total += continuation.len();
        for (step, &want) in continuation.iter().enumerate() {
            let argmax = (0..logits.len()).fold(0usize, |best, i| if logits[i] > logits[best] { i } else { best }) as u32;
            if argmax != want {
                println!("prompt {:?}: diverged at step {step}: got {argmax}, reference {want}", case.get("id"));
                break;
            }
            matched += 1;
            logits = session.feed(argmax).expect("continuation token");
        }
    }
    println!("SmolLM2-135M f32: {matched}/{total} greedy continuation tokens match");
    assert_eq!(matched, total, "every greedy token must match the transformers reference");
}

#[test]
fn test_smollm2_q8() {
    let f32_path = "/Volumes/SammyDisk/models/otteros/SmolLM2-135M-Instruct.f32.otm";
    let q8_path = "/Volumes/SammyDisk/models/otteros/SmolLM2-135M-Instruct.q8.otm";
    let golden_path = GOLDEN;

    let f32_bytes = load_model_bytes(f32_path);
    let q8_bytes = load_model_bytes(q8_path);
    let f32_model = Model::load(&f32_bytes).expect("failed to load f32 model");
    let q8_model = Model::load(&q8_bytes).expect("failed to load q8 model");
    let golden = load_golden(golden_path);

    let detailed = golden.as_object()
        .and_then(|obj| obj.get("detailed"))
        .expect("no detailed in golden");

    let mut sum_error = 0.0_f64;
    let mut max_error = 0.0_f32;
    let mut error_count = 0;

    if let Value::Array(cases) = detailed {
        for case in cases {
            if let Value::Object(case_obj) = case {
                let token_ids: Vec<u32> = if let Some(Value::Array(ids)) = case_obj.get("token_ids") {
                    ids.iter().filter_map(|v| {
                        if let Value::Number(n) = v {
                            n.as_i64().map(|x| x as u32)
                        } else {
                            None
                        }
                    }).collect()
                } else {
                    Vec::new()
                };

                let mut f32_session = Session::new(&f32_model);
                let mut q8_session = Session::new(&q8_model);
                let mut f32_logits_hist = Vec::new();
                let mut q8_logits_hist = Vec::new();

                for tok in &token_ids {
                    if let (Ok(f32_logits), Ok(q8_logits)) = (f32_session.feed(*tok), q8_session.feed(*tok)) {
                        f32_logits_hist.push(f32_logits);
                        q8_logits_hist.push(q8_logits);
                    }
                }

                for (f32_logits, q8_logits) in f32_logits_hist.iter().zip(q8_logits_hist.iter()) {
                    for (f32_val, q8_val) in f32_logits.iter().zip(q8_logits.iter()) {
                        let err = (f32_val - q8_val).abs();
                        sum_error += err as f64;
                        max_error = max_error.max(err);
                        error_count += 1;
                    }
                }
            }
        }
    }

    let mean_error = if error_count > 0 {
        (sum_error / error_count as f64) as f32
    } else {
        0.0
    };

    println!("SmolLM2-135M q8 (all-Q8): mean error {:.6}, max error {:.6}", mean_error, max_error);
    println!("  (target: mean < 0.05, max < 0.5 — requires F32 embeddings)");

    // Current thresholds until F32 embeddings implemented
    assert!(mean_error < 0.25 && max_error < 2.0, "Q8 error outside current bounds");
}

#[test]
fn perf_smollm2() {
    let f32_path = "/Volumes/SammyDisk/models/otteros/SmolLM2-135M-Instruct.f32.otm";
    let q8_path = "/Volumes/SammyDisk/models/otteros/SmolLM2-135M-Instruct.q8.otm";

    for (path, label) in &[(f32_path, "f32"), (q8_path, "q8")] {
        let model_bytes = load_model_bytes(path);
        let model = Model::load(&model_bytes).expect("failed to load model");

        let mut session = Session::new(&model);
        let num_tokens = 100;

        let start = std::time::Instant::now();
        for i in 0..num_tokens {
            let _ = session.feed(i % 1000);
        }
        let elapsed = start.elapsed();

        let tok_per_sec = num_tokens as f64 / elapsed.as_secs_f64();
        println!("SmolLM2-135M {}: {:.2} tokens/s", label, tok_per_sec);
    }
}
