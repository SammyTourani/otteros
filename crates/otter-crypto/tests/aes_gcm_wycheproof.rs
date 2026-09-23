//! Project Wycheproof's AES-GCM test vectors
//! (`scripts/gen-crypto-vectors.py`'s `aes{128,256}_gcm_wycheproof.txt`, the
//! 96-bit-IV/128-bit-tag groups only -- see that script's docstring for why
//! the other groups, which test non-96-bit IV sizes and the AES-192 key size
//! this crate does not implement, are skipped). Both key sizes always run
//! through `Aes128Gcm::new_software`/`Aes256Gcm::new_software` here (the
//! hardware/software differential test in `hw_sw_differential.rs` is what
//! exercises the AES-NI/PCLMULQDQ path); the assertions are otherwise the
//! same shape as `chacha20_poly1305_wycheproof.rs`'s.

mod common;

use common::{hex_decode, hex_encode, read_fixture};
use otter_crypto::{Aead, AeadError, Aes128Gcm, Aes256Gcm};

fn run_fixture<C: Aead>(fixture: &str, key_len: usize, new_cipher: impl Fn(&[u8]) -> C) {
    let rows = read_fixture(fixture);
    assert!(rows.len() >= 50, "expected a substantial number of cases in {fixture}, got {}", rows.len());

    let mut valid = 0;
    let mut invalid = 0;
    for row in &rows {
        let [tcid, result, key_hex, iv_hex, aad_hex, msg_hex, ct_hex, tag_hex] = row.as_slice() else {
            panic!("row has {} fields, expected 8: {row:?}", row.len());
        };
        let key = hex_decode(key_hex);
        assert_eq!(key.len(), key_len, "tcId={tcid}: unexpected key length");
        let nonce: [u8; 12] = hex_decode(iv_hex).try_into().unwrap();
        let aad = hex_decode(aad_hex);
        let msg = hex_decode(msg_hex);
        let ct = hex_decode(ct_hex);
        let tag: [u8; 16] = hex_decode(tag_hex).try_into().unwrap();
        let cipher = new_cipher(&key);

        match result.as_str() {
            "valid" => {
                valid += 1;
                let mut buf = msg.clone();
                let computed_tag = cipher.seal_in_place(&nonce, &aad, &mut buf);
                assert_eq!(hex_encode(&buf), *ct_hex, "tcId={tcid}: ciphertext mismatch");
                assert_eq!(hex_encode(&computed_tag), *tag_hex, "tcId={tcid}: tag mismatch");

                let mut open_buf = ct.clone();
                cipher
                    .open_in_place(&nonce, &aad, &mut open_buf, &tag)
                    .unwrap_or_else(|e| panic!("tcId={tcid}: expected valid tag to open, got {e:?}"));
                assert_eq!(open_buf, msg, "tcId={tcid}: opened plaintext mismatch");
            }
            "invalid" => {
                invalid += 1;
                let mut open_buf = ct.clone();
                let err = cipher
                    .open_in_place(&nonce, &aad, &mut open_buf, &tag)
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
    assert!(valid >= 10, "expected several valid cases in {fixture}, got {valid}");
    assert!(invalid >= 1, "expected at least one invalid case in {fixture}, got {invalid}");
}

#[test]
fn aes128_gcm_wycheproof() {
    run_fixture("aes128_gcm_wycheproof.txt", 16, |k| Aes128Gcm::new_software(&k.try_into().unwrap()));
}

#[test]
fn aes256_gcm_wycheproof() {
    run_fixture("aes256_gcm_wycheproof.txt", 32, |k| Aes256Gcm::new_software(&k.try_into().unwrap()));
}
