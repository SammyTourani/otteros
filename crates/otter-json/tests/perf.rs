//! Performance smoke test (brief M7-T1: "fast enough to parse a 10 MB
//! tokenizer.json in well under a second on the host"). Builds a synthetic
//! document shaped like a real `tokenizer.json` (a huge flat vocab object
//! mapping strings to small integers, plus a big array of merge-rule
//! string pairs) and times a single parse. The assertion threshold is kept
//! generous (2s) to avoid flakes under host load (CLAUDE.md); the actual
//! time is printed for the human/agent reading the test run.

use std::time::Instant;

use otter_json::parse;

fn build_synthetic_tokenizer_json(target_bytes: usize) -> String {
    let mut s = String::with_capacity(target_bytes + 4096);
    s.push_str(r#"{"version":"1.0","model":{"type":"BPE","vocab":{"#);
    let mut id = 0u32;
    while s.len() < target_bytes * 3 / 4 {
        if id > 0 {
            s.push(',');
        }
        // A mix of ASCII and multi-byte UTF-8 to exercise the whole string path.
        s.push_str(&format!(r#""token_{id}_éü":{id}"#));
        id += 1;
    }
    s.push_str(r#"},"merges":["#);
    let mut m = 0u32;
    let mut first = true;
    while s.len() < target_bytes {
        if !first {
            s.push(',');
        }
        first = false;
        s.push_str(&format!(r#"["piece_a_{m}","piece_b_{m}"]"#));
        m += 1;
    }
    s.push_str("]}}");
    s
}

#[test]
fn parses_a_ten_megabyte_document_in_well_under_a_second() {
    let doc = build_synthetic_tokenizer_json(10 * 1024 * 1024);
    assert!(doc.len() >= 10 * 1024 * 1024, "test document should be >= 10 MB, was {}", doc.len());

    let start = Instant::now();
    let value = parse(doc.as_bytes()).expect("synthetic document must parse");
    let elapsed = start.elapsed();

    println!(
        "otter-json perf: parsed {:.2} MB in {:?} ({:.1} MB/s)",
        doc.len() as f64 / (1024.0 * 1024.0),
        elapsed,
        (doc.len() as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64()
    );

    let vocab = value.get("model").unwrap().get("vocab").unwrap().as_object().unwrap();
    assert!(vocab.len() > 10_000);
    assert!(elapsed.as_secs_f64() < 2.0, "parse took {elapsed:?}, expected well under a second");
}
