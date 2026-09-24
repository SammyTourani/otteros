//! Project Wycheproof's RSASSA-PSS test vectors
//! (`scripts/gen-crypto-vectors.py`'s `rsa_pss_2048_sha256_wycheproof.txt`/
//! `rsa_pss_4096_sha512_wycheproof.txt`, converted from the upstream
//! `rsa_pss_2048_sha256_mgf1_32_test.json`/`rsa_pss_4096_sha512_mgf1_64_test.json`
//! -- chosen, per the brief, specifically because their salt length (32/64)
//! equals their hash's output length, this crate's only supported PSS
//! configuration (TLS 1.3's own requirement, RFC 8446 section 4.2.3)).

mod common;

use common::{hex_decode, read_fixture};
use otter_crypto::hash::Digest;
use otter_crypto::rsa::{HashAlg, PublicKey};
use otter_crypto::sha256::Sha256;
use otter_crypto::sha512::Sha512;

fn run(fixture: &str, alg: HashAlg, digest_of: impl Fn(&[u8]) -> Digest, min_cases: usize) {
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
        let digest = digest_of(&msg);
        let outcome = key.verify_pss(alg, &digest, &sig);

        match result.as_str() {
            "valid" => {
                valid += 1;
                outcome.unwrap_or_else(|err| panic!("{fixture} tcId={tcid}: expected valid, got {err:?}"));
            }
            "invalid" => {
                invalid += 1;
                assert!(outcome.is_err(), "{fixture} tcId={tcid}: expected invalid signature to be rejected");
            }
            "acceptable" => {}
            other => panic!("{fixture} tcId={tcid}: unknown result {other:?}"),
        }
    }
    assert!(valid >= 5, "{fixture}: expected several valid cases, got {valid}");
    assert!(invalid >= 5, "{fixture}: expected several invalid cases, got {invalid}");
}

#[test]
fn rsa_pss_2048_sha256_wycheproof() {
    run("rsa_pss_2048_sha256_wycheproof.txt", HashAlg::Sha256, |m| Digest::from_slice(&Sha256::digest(m)), 50);
}

#[test]
fn rsa_pss_4096_sha512_wycheproof() {
    run("rsa_pss_4096_sha512_wycheproof.txt", HashAlg::Sha512, |m| Digest::from_slice(&Sha512::digest(m)), 100);
}
