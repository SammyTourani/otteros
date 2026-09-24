//! Project Wycheproof's `ecdsa_secp256r1_sha256_test` vectors
//! (`scripts/gen-crypto-vectors.py`'s `ecdsa_p256_sha256_wycheproof.txt`).
//! Covers strict-DER parsing (`BerEncodedSignature`, `MissingZero`, ...),
//! range checks (`RangeCheck`, `SmallRandS`, ...) and the complete addition
//! formula's exception-freeness (`PointDuplication`, `EdgeCaseShamirMultiplication`).

mod common;

use common::{hex_decode, read_fixture};
use otter_crypto::p256::PublicKey;
use otter_crypto::sha256::Sha256;

#[test]
fn ecdsa_p256_sha256_wycheproof() {
    let rows = read_fixture("ecdsa_p256_sha256_wycheproof.txt");
    assert!(rows.len() >= 400, "expected a substantial number of cases, got {}", rows.len());

    let mut valid = 0;
    let mut invalid = 0;
    for row in &rows {
        let [tcid, result, flags, pubkey_hex, msg_hex, sig_hex] = row.as_slice() else {
            panic!("row has {} fields, expected 6: {row:?}", row.len());
        };
        let pubkey_bytes = hex_decode(pubkey_hex);
        let msg = hex_decode(msg_hex);
        let sig = hex_decode(sig_hex);
        let digest = Sha256::digest(&msg);

        let outcome = PublicKey::from_sec1(&pubkey_bytes).and_then(|key| key.verify(&digest, &sig));
        match result.as_str() {
            "valid" => {
                valid += 1;
                outcome.unwrap_or_else(|err| panic!("tcId={tcid} flags={flags}: expected valid, got {err:?}"));
            }
            "invalid" => {
                invalid += 1;
                assert!(outcome.is_err(), "tcId={tcid} flags={flags}: expected invalid signature to be rejected");
            }
            "acceptable" => {}
            other => panic!("tcId={tcid}: unknown result {other:?}"),
        }
    }
    assert!(valid >= 100, "expected many valid cases, got {valid}");
    assert!(invalid >= 100, "expected many invalid cases, got {invalid}");
}
