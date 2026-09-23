//! Project Wycheproof's HMAC-SHA-256/384/512 test vectors
//! (`scripts/gen-crypto-vectors.py`'s `hmac_{sha256,sha384,sha512}_wycheproof.txt`,
//! converted from the upstream `hmac_sha*_test.json` files under
//! C2SP/wycheproof's `testvectors_v1/`). Covers full-length and truncated tags,
//! keys shorter/longer/exactly the block size, and Wycheproof's `ModifiedTag`
//! cases.

mod common;

use common::read_fixture;
use otter_crypto::{Hash, Hmac};

fn run<H: Hash>(fixture: &str) -> usize {
    let rows = read_fixture(fixture);
    assert!(!rows.is_empty(), "{fixture}: no test cases loaded");
    for row in &rows {
        let [tcid, result, key_hex, msg_hex, tag_hex] = row.as_slice() else {
            panic!("{fixture}: row has {} fields, expected 5: {row:?}", row.len());
        };
        let key = common::hex_decode(key_hex);
        let msg = common::hex_decode(msg_hex);
        let tag = common::hex_decode(tag_hex);

        let ok = Hmac::<H>::verify(&key, &msg, &tag);
        match result.as_str() {
            "valid" => assert!(ok, "{fixture} tcId={tcid}: expected valid tag to verify"),
            "invalid" => assert!(!ok, "{fixture} tcId={tcid}: expected invalid tag to be rejected"),
            other => panic!("{fixture} tcId={tcid}: unknown result {other:?}"),
        }
    }
    rows.len()
}

#[test]
fn hmac_sha256_wycheproof() {
    let n = run::<otter_crypto::Sha256>("hmac_sha256_wycheproof.txt");
    assert!(n >= 100, "expected a substantial number of HMAC-SHA-256 cases, got {n}");
}

#[test]
fn hmac_sha384_wycheproof() {
    let n = run::<otter_crypto::Sha384>("hmac_sha384_wycheproof.txt");
    assert!(n >= 100, "expected a substantial number of HMAC-SHA-384 cases, got {n}");
}

#[test]
fn hmac_sha512_wycheproof() {
    let n = run::<otter_crypto::Sha512>("hmac_sha512_wycheproof.txt");
    assert!(n >= 100, "expected a substantial number of HMAC-SHA-512 cases, got {n}");
}
