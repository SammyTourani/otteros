//! Project Wycheproof's RSASSA-PKCS1-v1_5 test vectors
//! (`scripts/gen-crypto-vectors.py`'s
//! `rsa_pkcs1v15_{2048,3072,4096}_sha256_wycheproof.txt`, converted from the
//! upstream `rsa_signature_{2048,3072,4096}_sha256_test.json` files, whole --
//! unlike the AEAD fixtures, these have no parameter-size groups to filter,
//! every group is a real (if sometimes small-exponent) RSA key). Each file's
//! own `msg` is hashed with SHA-256 here to produce the `digest` this crate's
//! API takes; the signature is checked byte-for-byte against a freshly
//! rebuilt expected EMSA-PKCS1-v1_5 encoding (see `src/rsa.rs`).

mod common;

use common::{hex_decode, read_fixture};
use otter_crypto::hash::Digest;
use otter_crypto::rsa::{HashAlg, PublicKey};
use otter_crypto::sha256::Sha256;

fn run(fixture: &str, min_cases: usize) {
    let rows = read_fixture(fixture);
    assert!(rows.len() >= min_cases, "{fixture}: expected at least {min_cases} cases, got {}", rows.len());

    let mut valid = 0;
    let mut invalid = 0;
    for row in &rows {
        let [tcid, result, n_hex, e_hex, msg_hex, sig_hex] = row.as_slice() else {
            panic!("{fixture}: row has {} fields, expected 6: {row:?}", row.len());
        };
        let n = hex_decode(n_hex);
        let e = hex_decode(e_hex);
        let msg = hex_decode(msg_hex);
        let sig = hex_decode(sig_hex);

        let key = PublicKey::from_components(&n, &e)
            .unwrap_or_else(|err| panic!("{fixture} tcId={tcid}: valid Wycheproof key rejected: {err:?}"));
        let digest = Digest::from_slice(&Sha256::digest(&msg));
        let outcome = key.verify_pkcs1v15(HashAlg::Sha256, &digest, &sig);

        match result.as_str() {
            "valid" => {
                valid += 1;
                outcome.unwrap_or_else(|err| panic!("{fixture} tcId={tcid}: expected valid, got {err:?}"));
            }
            "invalid" => {
                invalid += 1;
                assert!(outcome.is_err(), "{fixture} tcId={tcid}: expected invalid signature to be rejected");
            }
            "acceptable" => {} // Not produced by this Wycheproof file, but handled just in case.
            other => panic!("{fixture} tcId={tcid}: unknown result {other:?}"),
        }
    }
    assert!(valid >= 5, "{fixture}: expected several valid cases, got {valid}");
    assert!(invalid >= 5, "{fixture}: expected several invalid cases, got {invalid}");
}

#[test]
fn rsa_pkcs1v15_2048_sha256_wycheproof() {
    run("rsa_pkcs1v15_2048_sha256_wycheproof.txt", 200);
}

#[test]
fn rsa_pkcs1v15_3072_sha256_wycheproof() {
    run("rsa_pkcs1v15_3072_sha256_wycheproof.txt", 200);
}

#[test]
fn rsa_pkcs1v15_4096_sha256_wycheproof() {
    run("rsa_pkcs1v15_4096_sha256_wycheproof.txt", 200);
}
