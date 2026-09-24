//! Runs the upstream JSONTestSuite parsing corpus (github.com/nst/JSONTestSuite)
//! against otter-json's parser (brief M7-T1): every `y_*.json` must parse
//! successfully, every `n_*.json` must be rejected, and `i_*.json` cases
//! (implementation-defined by the RFC) are only tallied, never asserted on --
//! any deterministic accept/reject is correct as long as the parser doesn't
//! crash or hang.
//!
//! Needs `scripts/fetch-jsontestsuite.sh` to have been run first; fails
//! loudly (not silently) if it hasn't, matching otter-gfx's
//! tests/truetype.rs convention for fetched fixtures.

use std::fs;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    let dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/JSONTestSuite/test_parsing");
    if !dir.is_dir() {
        panic!("{} not found; run scripts/fetch-jsontestsuite.sh first", dir.display());
    }
    dir
}

#[test]
fn json_test_suite_y_and_n_cases_are_labelled_correctly() {
    let dir = corpus_dir();
    let mut entries: Vec<PathBuf> =
        fs::read_dir(&dir).expect("read test_parsing dir").map(|e| e.expect("dir entry").path()).collect();
    entries.sort();
    assert!(entries.len() > 300, "expected the full ~318-file corpus, found {}", entries.len());

    let (mut y_total, mut n_total) = (0u32, 0u32);
    let mut y_wrongly_rejected = Vec::new();
    let mut n_wrongly_accepted = Vec::new();
    let (mut i_accepted, mut i_rejected) = (0u32, 0u32);

    for path in &entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".json") {
            continue;
        }
        let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let accepted = otter_json::parse(&bytes).is_ok();

        if let Some(rest) = name.strip_prefix("y_") {
            y_total += 1;
            if !accepted {
                y_wrongly_rejected.push(rest.to_string());
            }
        } else if let Some(rest) = name.strip_prefix("n_") {
            n_total += 1;
            if accepted {
                n_wrongly_accepted.push(rest.to_string());
            }
        } else if name.starts_with("i_") {
            if accepted {
                i_accepted += 1;
            } else {
                i_rejected += 1;
            }
        }
    }

    println!(
        "JSONTestSuite: y_ {}/{y_total} accepted, n_ {}/{n_total} rejected, \
         i_ {i_accepted} accepted / {i_rejected} rejected (implementation-defined, not asserted)",
        y_total - y_wrongly_rejected.len() as u32,
        n_total - n_wrongly_accepted.len() as u32,
    );

    assert!(y_wrongly_rejected.is_empty(), "y_ cases wrongly rejected: {y_wrongly_rejected:?}");
    assert!(n_wrongly_accepted.is_empty(), "n_ cases wrongly accepted: {n_wrongly_accepted:?}");
}
