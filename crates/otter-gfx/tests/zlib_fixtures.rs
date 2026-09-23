//! Runs every fixture `scripts/gen-gfx-fixtures.py` wrote to
//! `tests/fixtures/*.zlib` through [`otter_gfx::inflate::inflate_zlib`]: valid
//! streams (covering stored, fixed-Huffman, dynamic-Huffman and multi-block
//! encodings) must decompress to exactly the paired `.expected` bytes; streams
//! named `zlib_bad_*` are deliberately corrupted and must be rejected, never
//! panic.

use otter_gfx::inflate::inflate_zlib;

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn zlib_fixture_names() -> Vec<String> {
    let dir = fixtures_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read {dir:?} ({e}); run scripts/gen-gfx-fixtures.py first"))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".zlib"))
        .map(|name| name.trim_end_matches(".zlib").to_string())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no .zlib fixtures found in {dir:?}; run scripts/gen-gfx-fixtures.py");
    names
}

#[test]
fn every_good_fixture_decompresses_to_its_expected_bytes() {
    let mut checked = 0;
    for name in zlib_fixture_names() {
        if name.starts_with("zlib_bad_") {
            continue;
        }
        let dir = fixtures_dir();
        let stream = std::fs::read(dir.join(format!("{name}.zlib"))).unwrap();
        let expected = std::fs::read(dir.join(format!("{name}.expected"))).unwrap();
        let got = inflate_zlib(&stream).unwrap_or_else(|e| panic!("{name}: inflate failed: {e:?}"));
        assert_eq!(got, expected, "fixture {name} decompressed to the wrong bytes");
        checked += 1;
    }
    assert!(checked >= 5, "expected several good zlib fixtures, only checked {checked}");
}

#[test]
fn every_corrupted_fixture_is_rejected_not_panicking() {
    let mut checked = 0;
    for name in zlib_fixture_names() {
        if !name.starts_with("zlib_bad_") {
            continue;
        }
        let stream = std::fs::read(fixtures_dir().join(format!("{name}.zlib"))).unwrap();
        assert!(inflate_zlib(&stream).is_err(), "fixture {name} should have been rejected");
        checked += 1;
    }
    assert!(checked >= 3, "expected several corrupted zlib fixtures, only checked {checked}");
}
