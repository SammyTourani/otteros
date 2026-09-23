//! Runs every fixture `scripts/gen-gfx-fixtures.py` wrote to
//! `tests/fixtures/*.png` through [`otter_gfx::png::decode`]: every colour
//! type/bit depth/filter/tRNS/palette combination must decode to exactly the
//! premultiplied-ARGB pixels recorded in the paired `.json` file;
//! `interlaced_adam7.png` must be rejected as unsupported, and `bad_crc.png`
//! must be rejected for its checksum -- neither should ever panic.
//!
//! `.json` files are a fixed, simple shape this crate's own generator writes
//! (`{"width": W, "height": H, "argb": [...]}`), so a tiny hand-rolled reader
//! is used instead of a JSON dependency (this crate has none -- DECISIONS.md
//! D27).

use otter_gfx::png::{decode, PngError};

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

struct Expected {
    width: usize,
    height: usize,
    argb: Vec<u32>,
}

fn extract_number(json: &str, key: &str) -> u64 {
    let idx = json.find(key).unwrap_or_else(|| panic!("missing {key} in fixture json")) + key.len();
    let rest = json[idx..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().unwrap_or_else(|_| panic!("bad number after {key}"))
}

fn parse_expected(json: &str) -> Expected {
    let width = extract_number(json, "\"width\":") as usize;
    let height = extract_number(json, "\"height\":") as usize;
    let key = "\"argb\":";
    let start = json.find(key).expect("missing argb key") + key.len();
    let bracket_start = json[start..].find('[').unwrap() + start + 1;
    let bracket_end = json[bracket_start..].find(']').unwrap() + bracket_start;
    let body = json[bracket_start..bracket_end].trim();
    let argb = if body.is_empty() {
        Vec::new()
    } else {
        body.split(',').map(|s| s.trim().parse::<u64>().unwrap() as u32).collect()
    };
    Expected { width, height, argb }
}

fn png_fixture_stems() -> Vec<String> {
    let dir = fixtures_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read {dir:?} ({e}); run scripts/gen-gfx-fixtures.py first"))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".png"))
        .map(|name| name.trim_end_matches(".png").to_string())
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no .png fixtures found in {dir:?}; run scripts/gen-gfx-fixtures.py");
    names
}

#[test]
fn every_fixture_with_a_json_sidecar_decodes_to_exact_expected_pixels() {
    let mut checked = 0;
    for stem in png_fixture_stems() {
        let dir = fixtures_dir();
        if stem == "interlaced_adam7" || stem == "bad_crc" {
            continue; // handled by their own must-fail tests below
        }
        let json_path = dir.join(format!("{stem}.json"));
        if !json_path.exists() {
            continue;
        }
        let png_bytes = std::fs::read(dir.join(format!("{stem}.png"))).unwrap();
        let json = std::fs::read_to_string(&json_path).unwrap();
        let expected = parse_expected(&json);

        let surface = decode(&png_bytes).unwrap_or_else(|e| panic!("{stem}: decode failed: {e:?}"));
        assert_eq!(surface.w, expected.width, "{stem}: width mismatch");
        assert_eq!(surface.h, expected.height, "{stem}: height mismatch");
        assert_eq!(surface.px.len(), expected.argb.len(), "{stem}: pixel count mismatch");
        for (i, (&got, &want)) in surface.px.iter().zip(expected.argb.iter()).enumerate() {
            assert_eq!(got, want, "{stem}: pixel {i} = {got:#010x}, want {want:#010x}");
        }
        checked += 1;
    }
    assert!(checked >= 10, "expected many PNG fixtures with exact pixels, only checked {checked}");
}

#[test]
fn adam7_interlaced_fixture_is_rejected() {
    let bytes = std::fs::read(fixtures_dir().join("interlaced_adam7.png")).unwrap();
    assert_eq!(decode(&bytes), Err(PngError::Adam7NotSupported));
}

#[test]
fn bad_crc_fixture_is_rejected() {
    let bytes = std::fs::read(fixtures_dir().join("bad_crc.png")).unwrap();
    assert_eq!(decode(&bytes), Err(PngError::CrcMismatch));
}
