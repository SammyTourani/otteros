//! Project Wycheproof's HKDF-SHA-256/384/512 test vectors
//! (`scripts/gen-crypto-vectors.py`'s `hkdf_{sha256,sha384,sha512}_wycheproof.txt`).
//! Every `invalid` case here is Wycheproof's `SizeTooLarge`: a requested output
//! length one byte past `255 * HashLen`, which RFC 5869 section 2.3 says
//! HKDF-Expand must refuse.

mod common;

use common::read_fixture;
use otter_crypto::{hkdf_expand, hkdf_extract, Hash, HkdfError};

fn run<H: Hash>(fixture: &str) -> usize {
    let rows = read_fixture(fixture);
    assert!(!rows.is_empty(), "{fixture}: no test cases loaded");
    for row in &rows {
        let [tcid, result, ikm_hex, salt_hex, info_hex, size_s, okm_hex] = row.as_slice() else {
            panic!("{fixture}: row has {} fields, expected 7: {row:?}", row.len());
        };
        let ikm = common::hex_decode(ikm_hex);
        let salt = common::hex_decode(salt_hex);
        let info = common::hex_decode(info_hex);
        let size: usize = size_s.parse().unwrap();

        let prk = hkdf_extract::<H>(&salt, &ikm);
        let mut okm = alloc_zeroed(size);
        let outcome = hkdf_expand::<H>(prk.as_bytes(), &info, &mut okm);

        match result.as_str() {
            "valid" => {
                outcome.unwrap_or_else(|e| panic!("{fixture} tcId={tcid}: expected Ok, got {e:?}"));
                assert_eq!(common::hex_encode(&okm), *okm_hex, "{fixture} tcId={tcid}: OKM mismatch");
            }
            "invalid" => {
                assert_eq!(
                    outcome,
                    Err(HkdfError::OutputTooLong),
                    "{fixture} tcId={tcid}: expected OutputTooLong"
                );
            }
            other => panic!("{fixture} tcId={tcid}: unknown result {other:?}"),
        }
    }
    rows.len()
}

// A separate helper only so a huge (but still tiny in absolute terms, at most a
// few tens of KB) `SizeTooLarge` case doesn't need a fallible allocation dance.
fn alloc_zeroed(len: usize) -> Vec<u8> {
    vec![0u8; len]
}

#[test]
fn hkdf_sha256_wycheproof() {
    let n = run::<otter_crypto::Sha256>("hkdf_sha256_wycheproof.txt");
    assert!(n >= 50, "expected a substantial number of HKDF-SHA-256 cases, got {n}");
}

#[test]
fn hkdf_sha384_wycheproof() {
    let n = run::<otter_crypto::Sha384>("hkdf_sha384_wycheproof.txt");
    assert!(n >= 50, "expected a substantial number of HKDF-SHA-384 cases, got {n}");
}

#[test]
fn hkdf_sha512_wycheproof() {
    let n = run::<otter_crypto::Sha512>("hkdf_sha512_wycheproof.txt");
    assert!(n >= 50, "expected a substantial number of HKDF-SHA-512 cases, got {n}");
}
