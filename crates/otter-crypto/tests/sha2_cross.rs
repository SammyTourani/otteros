//! Cross-checks SHA-256/384/512 against Python's `hashlib` for a seeded-random
//! message at every length 0..=1100 bytes (`scripts/gen-crypto-vectors.py`'s
//! `sha2_cross.txt`). Message *content* diversity does not matter here -- only
//! hitting every length relative to each hash's block size (64 bytes for
//! SHA-256, 128 for SHA-384/512) does, since that is what exercises the padding
//! logic at every possible block-boundary offset.

mod common;

use common::{hex_decode, hex_encode, read_fixture};
use otter_crypto::{Sha256, Sha384, Sha512};

#[test]
fn hashlib_cross_check_every_length_0_to_1100() {
    let rows = read_fixture("sha2_cross.txt");
    assert!(rows.len() >= 1101, "expected 1101 rows (lengths 0..=1100), got {}", rows.len());

    for row in &rows {
        let [len_s, message_hex, sha256_hex, sha384_hex, sha512_hex] = row.as_slice() else {
            panic!("sha2_cross.txt row has {} fields, expected 5: {row:?}", row.len());
        };
        let len: usize = len_s.parse().unwrap();
        let message = hex_decode(message_hex);
        assert_eq!(message.len(), len, "length field disagrees with message_hex at len={len}");

        assert_eq!(hex_encode(&Sha256::digest(&message)), *sha256_hex, "SHA-256 mismatch at len={len}");
        assert_eq!(hex_encode(&Sha384::digest(&message)), *sha384_hex, "SHA-384 mismatch at len={len}");
        assert_eq!(hex_encode(&Sha512::digest(&message)), *sha512_hex, "SHA-512 mismatch at len={len}");
    }
}
