//! Project Wycheproof's ChaCha20-Poly1305 test vectors
//! (`scripts/gen-crypto-vectors.py`'s `chacha20_poly1305_wycheproof.txt`, the
//! 316-case standard-size group only -- see that script's docstring for why the
//! other nine groups, which test non-standard nonce/key/tag sizes, are skipped).
//! For `valid` cases, sealing the plaintext must reproduce the given ciphertext
//! and tag exactly (ChaCha20-Poly1305 is deterministic) and opening the
//! ciphertext must recover the plaintext; for `invalid` cases (tampered tag,
//! ciphertext or Poly1305 edge cases), opening must fail and must leave the
//! buffer zeroed.

mod common;

use common::{hex_decode, hex_encode, read_fixture};
use otter_crypto::{open_in_place, seal_in_place, AeadError};

#[test]
fn chacha20_poly1305_wycheproof() {
    let rows = read_fixture("chacha20_poly1305_wycheproof.txt");
    assert!(rows.len() >= 200, "expected a substantial number of AEAD cases, got {}", rows.len());

    let mut valid = 0;
    let mut invalid = 0;
    for row in &rows {
        let [tcid, result, key_hex, iv_hex, aad_hex, msg_hex, ct_hex, tag_hex] = row.as_slice() else {
            panic!("row has {} fields, expected 8: {row:?}", row.len());
        };
        let key: [u8; 32] = hex_decode(key_hex).try_into().unwrap();
        let iv: [u8; 12] = hex_decode(iv_hex).try_into().unwrap();
        let aad = hex_decode(aad_hex);
        let msg = hex_decode(msg_hex);
        let ct = hex_decode(ct_hex);
        let tag: [u8; 16] = hex_decode(tag_hex).try_into().unwrap();

        match result.as_str() {
            "valid" => {
                valid += 1;
                let mut buf = msg.clone();
                let computed_tag = seal_in_place(&key, &iv, &aad, &mut buf);
                assert_eq!(hex_encode(&buf), *ct_hex, "tcId={tcid}: ciphertext mismatch");
                assert_eq!(hex_encode(&computed_tag), *tag_hex, "tcId={tcid}: tag mismatch");

                let mut open_buf = ct.clone();
                open_in_place(&key, &iv, &aad, &mut open_buf, &tag)
                    .unwrap_or_else(|e| panic!("tcId={tcid}: expected valid tag to open, got {e:?}"));
                assert_eq!(open_buf, msg, "tcId={tcid}: opened plaintext mismatch");
            }
            "invalid" => {
                invalid += 1;
                let mut open_buf = ct.clone();
                let err = open_in_place(&key, &iv, &aad, &mut open_buf, &tag)
                    .expect_err(&format!("tcId={tcid}: expected invalid tag to be rejected"));
                assert_eq!(err, AeadError::InvalidTag);
                assert!(
                    open_buf.iter().all(|&b| b == 0),
                    "tcId={tcid}: buffer must be zeroed after a rejected tag"
                );
            }
            other => panic!("tcId={tcid}: unknown result {other:?}"),
        }
    }
    assert!(valid >= 100, "expected a substantial number of valid cases, got {valid}");
    assert!(invalid >= 10, "expected a substantial number of invalid cases, got {invalid}");
}
