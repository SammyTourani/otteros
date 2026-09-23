//! HKDF (RFC 5869), generic over [`Hash`].

use crate::hash::{Digest, Hash};
use crate::hmac::Hmac;

/// Errors [`hkdf_expand`] can return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HkdfError {
    /// RFC 5869 section 2.3: the requested output is longer than `255 *
    /// HashLen`, the most HKDF-Expand can ever produce.
    OutputTooLong,
}

/// RFC 5869 section 2.2, HKDF-Extract: `PRK = HMAC-Hash(salt, IKM)`. Per the RFC,
/// an absent salt is a string of `HashLen` zero bytes -- pass `&[]` for `salt` to
/// get exactly that (HMAC treats a short key by zero-padding it to the block
/// size, so an empty salt behaves the same as the RFC's "if not provided").
pub fn hkdf_extract<H: Hash>(salt: &[u8], ikm: &[u8]) -> Digest {
    Hmac::<H>::mac(salt, ikm)
}

/// RFC 5869 section 2.3, HKDF-Expand: fills `okm` (of any length up to `255 *
/// H::OUTPUT_SIZE`) from `prk` (as produced by [`hkdf_extract`]) and `info`.
pub fn hkdf_expand<H: Hash>(prk: &[u8], info: &[u8], okm: &mut [u8]) -> Result<(), HkdfError> {
    let hash_len = H::OUTPUT_SIZE;
    if okm.len() > 255 * hash_len {
        return Err(HkdfError::OutputTooLong);
    }

    let mut t_prev = Digest::from_slice(&[]); // T(0) = empty string.
    let mut counter: u8 = 1;
    let mut written = 0;
    while written < okm.len() {
        let mut h = Hmac::<H>::new(prk);
        h.update(t_prev.as_bytes());
        h.update(info);
        h.update(&[counter]);
        let t = h.finalize();

        let take = (okm.len() - written).min(hash_len);
        okm[written..written + take].copy_from_slice(&t.as_bytes()[..take]);
        written += take;
        t_prev = t;
        // `counter` reaches 255 only on the loop's last iteration (bounded by
        // the `OutputTooLong` check above), so this never needs to be read again.
        counter = counter.wrapping_add(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::Sha256;
    use crate::sha512::Sha512;
    use alloc::vec;
    use alloc::vec::Vec;

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex(bytes: &[u8]) -> alloc::string::String {
        use alloc::string::String;
        use core::fmt::Write;
        let mut s = String::new();
        for b in bytes {
            write!(s, "{b:02x}").unwrap();
        }
        s
    }

    // RFC 5869 appendix A.1 (SHA-256, basic case).
    #[test]
    fn rfc_5869_a1_sha256() {
        let ikm = hex_decode("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let salt = hex_decode("000102030405060708090a0b0c");
        let info = hex_decode("f0f1f2f3f4f5f6f7f8f9");
        let prk = hkdf_extract::<Sha256>(&salt, &ikm);
        assert_eq!(hex(&prk), "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5");

        let mut okm = [0u8; 42];
        hkdf_expand::<Sha256>(prk.as_bytes(), &info, &mut okm).unwrap();
        assert_eq!(
            hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    // RFC 5869 appendix A.2 (SHA-256, longer inputs/outputs).
    #[test]
    fn rfc_5869_a2_sha256_long() {
        let ikm = hex_decode(
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f404142434445464748494a4b4c4d4e4f",
        );
        let salt = hex_decode(
            "606162636465666768696a6b6c6d6e6f707172737475767778797a7b7c7d7e7f808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeaf",
        );
        let info = hex_decode(
            "b0b1b2b3b4b5b6b7b8b9babbbcbdbebfc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fafbfcfdfeff",
        );
        let prk = hkdf_extract::<Sha256>(&salt, &ikm);
        let mut okm = [0u8; 82];
        hkdf_expand::<Sha256>(prk.as_bytes(), &info, &mut okm).unwrap();
        assert_eq!(
            hex(&okm),
            "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87"
        );
    }

    // RFC 5869 appendix A.3 (SHA-256, zero-length salt/info).
    #[test]
    fn rfc_5869_a3_sha256_zero_length_salt() {
        let ikm = hex_decode("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let prk = hkdf_extract::<Sha256>(&[], &ikm);
        assert_eq!(hex(&prk), "19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04");
        let mut okm = [0u8; 42];
        hkdf_expand::<Sha256>(prk.as_bytes(), &[], &mut okm).unwrap();
        assert_eq!(
            hex(&okm),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
    }

    // RFC 5869 appendix A.4 (SHA-1 in the RFC; re-run with SHA-256/512 here since
    // this crate has no SHA-1 -- exercises the basic case end to end for SHA-512.
    #[test]
    fn hkdf_sha512_round_trips() {
        let ikm = hex_decode("0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b");
        let salt = hex_decode("000102030405060708090a0b0c");
        let info = hex_decode("f0f1f2f3f4f5f6f7f8f9");
        let prk = hkdf_extract::<Sha512>(&salt, &ikm);
        let mut okm = [0u8; 42];
        hkdf_expand::<Sha512>(prk.as_bytes(), &info, &mut okm).unwrap();
        // No independent published vector for this exact input with SHA-512, so
        // assert internal consistency instead: re-deriving with the same inputs
        // is deterministic, and a different salt must not collide.
        let mut okm2 = [0u8; 42];
        hkdf_expand::<Sha512>(prk.as_bytes(), &info, &mut okm2).unwrap();
        assert_eq!(okm, okm2);
        let prk_other_salt = hkdf_extract::<Sha512>(b"different salt", &ikm);
        let mut okm3 = [0u8; 42];
        hkdf_expand::<Sha512>(prk_other_salt.as_bytes(), &info, &mut okm3).unwrap();
        assert_ne!(okm, okm3);
    }

    #[test]
    fn output_length_zero_is_ok_and_empty() {
        let prk = hkdf_extract::<Sha256>(b"salt", b"ikm");
        let mut okm: [u8; 0] = [];
        assert!(hkdf_expand::<Sha256>(prk.as_bytes(), b"info", &mut okm).is_ok());
    }

    #[test]
    fn output_length_at_max_is_ok_beyond_is_error() {
        let prk = hkdf_extract::<Sha256>(b"salt", b"ikm");
        let mut at_max = vec![0u8; 255 * 32];
        assert!(hkdf_expand::<Sha256>(prk.as_bytes(), b"info", &mut at_max).is_ok());
        let mut over_max = vec![0u8; 255 * 32 + 1];
        assert_eq!(hkdf_expand::<Sha256>(prk.as_bytes(), b"info", &mut over_max), Err(HkdfError::OutputTooLong));
    }

    #[test]
    fn expand_matches_manual_concatenation_at_a_block_boundary() {
        // Requesting exactly N*HashLen bytes, and N*HashLen+1, should agree on
        // the first N*HashLen bytes (catches off-by-one errors in the T(i) loop).
        let prk = hkdf_extract::<Sha256>(b"salt", b"ikm");
        let mut a = [0u8; 64]; // exactly 2 * 32
        let mut b = [0u8; 65];
        hkdf_expand::<Sha256>(prk.as_bytes(), b"info", &mut a).unwrap();
        hkdf_expand::<Sha256>(prk.as_bytes(), b"info", &mut b).unwrap();
        assert_eq!(&a[..], &b[..64]);
    }
}
