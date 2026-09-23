//! Shared helpers for otter-crypto's integration tests: hex encode/decode and
//! parsing for the compact, pipe-delimited fixture files
//! `scripts/gen-crypto-vectors.py` writes to `tests/vectors/`.
//!
//! Lives at `tests/common/mod.rs` (a subdirectory, not `tests/common.rs`) so
//! cargo does not treat this file as its own top-level integration-test binary
//! -- the standard pattern for code shared between `tests/*.rs` files.
#![allow(dead_code)] // not every test file uses every helper.

use std::path::PathBuf;

pub fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

pub fn hex_decode(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0, "odd-length hex string: {s:?}");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or_else(|e| panic!("bad hex byte in {s:?}: {e}")))
        .collect()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

/// Reads a fixture file: `#`-prefixed header/comment lines are skipped, every
/// other non-empty line is one `|`-separated row.
pub fn read_fixture(name: &str) -> Vec<Vec<String>> {
    let path = vectors_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("could not read {path:?} ({e}); run scripts/gen-crypto-vectors.py first")
    });
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('|').map(str::to_string).collect())
        .collect()
}
