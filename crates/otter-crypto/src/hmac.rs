//! HMAC (RFC 2104, test vectors in RFC 4231), generic over [`Hash`] so the same
//! code serves HMAC-SHA-256 and HMAC-SHA-384/512.

use crate::ct;
use crate::hash::{Digest, Hash};

/// The largest block size among this crate's hashes (SHA-384/512's 128 bytes),
/// so `Hmac<H>` can keep its pad buffers as plain arrays instead of needing
/// `alloc` for every instance.
const MAX_BLOCK_SIZE: usize = 128;

/// Streaming HMAC. `Hmac::<H>::mac(key, data)` for one-shot use, or
/// `Hmac::<H>::verify(key, data, tag)` to check a received tag in constant time.
pub struct Hmac<H: Hash> {
    inner: H,
    opad_block: ct::Zeroizing<[u8; MAX_BLOCK_SIZE]>,
    block_size: usize,
}

impl<H: Hash> Hmac<H> {
    /// RFC 2104 section 2: keys longer than the block size are hashed down first;
    /// keys shorter than the block size are zero-padded. Builds `ipad`/`opad` from
    /// the (possibly hashed) key and primes the inner hash with `key ^ ipad`.
    pub fn new(key: &[u8]) -> Self {
        let block_size = H::BLOCK_SIZE;
        debug_assert!(block_size <= MAX_BLOCK_SIZE);

        let mut k0 = ct::Zeroizing::new([0u8; MAX_BLOCK_SIZE]);
        if key.len() > block_size {
            let digest = H::digest(key);
            k0[..digest.as_bytes().len()].copy_from_slice(digest.as_bytes());
        } else {
            k0[..key.len()].copy_from_slice(key);
        }

        let mut ipad_block = [0u8; MAX_BLOCK_SIZE];
        let mut opad_block = ct::Zeroizing::new([0u8; MAX_BLOCK_SIZE]);
        for i in 0..block_size {
            ipad_block[i] = k0[i] ^ 0x36;
            opad_block[i] = k0[i] ^ 0x5c;
        }

        let mut inner = H::new();
        inner.update(&ipad_block[..block_size]);
        // `ipad_block` held key material XORed with a public constant; wipe it
        // now that it has been absorbed into the inner hash's internal state
        // (best-effort, see `ct` module doc).
        ct::zeroize(&mut ipad_block);

        Hmac { inner, opad_block, block_size }
    }

    /// Feeds more input. May be called any number of times.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Consumes the HMAC state and returns the tag.
    pub fn finalize(self) -> Digest {
        let inner_digest = self.inner.finalize();
        let mut outer = H::new();
        outer.update(&self.opad_block[..self.block_size]);
        outer.update(inner_digest.as_bytes());
        outer.finalize()
    }

    /// One-shot: `Hmac::<H>::new(key)` + `update(data)` + `finalize()`.
    pub fn mac(key: &[u8], data: &[u8]) -> Digest {
        let mut h = Hmac::<H>::new(key);
        h.update(data);
        h.finalize()
    }

    /// Verifies a received `tag` (which may be truncated, as HMAC is sometimes
    /// used) against `key`/`data` in constant time. A `tag` longer than `H`'s
    /// output is always rejected.
    pub fn verify(key: &[u8], data: &[u8], tag: &[u8]) -> bool {
        let computed = Self::mac(key, data);
        if tag.len() > computed.as_bytes().len() {
            return false;
        }
        ct::ct_eq(&computed.as_bytes()[..tag.len()], tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::Sha256;
    use crate::sha512::{Sha384, Sha512};
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

    // RFC 4231 test cases 1, 2 and 7 (short key, short key+data, and a key/data
    // pair both longer than the block size, exercising the key-hashing path).
    struct Rfc4231Case {
        key: &'static str,
        data: &'static str,
        sha256: &'static str,
        sha384: &'static str,
        sha512: &'static str,
    }

    const RFC_4231_CASES: &[Rfc4231Case] = &[
        Rfc4231Case {
            key: "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
            data: "4869205468657265",
            sha256: "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            sha384: "afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6",
            sha512: "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854",
        },
        Rfc4231Case {
            key: "4a656665",
            data: "7768617420646f2079612077616e7420666f72206e6f7468696e673f",
            sha256: "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            sha384: "af45d2e376484031617f78d2b58a6b1b9c7ef464f5a01b47e42ec3736322445e8e2240ca5e69e2c78b3239ecfab21649",
            sha512: "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
        },
        Rfc4231Case {
            // RFC 4231 Test Case 7: key and data both longer than the SHA-384/512
            // block size (128 bytes), exercising HMAC's key-hashing path for
            // every hash in this crate (SHA-256's block size is only 64 bytes,
            // so this also covers it).
            key: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            data: "5468697320697320612074657374207573696e672061206c6172676572207468616e20626c6f636b2d73697a65206b657920616e642061206c6172676572207468616e20626c6f636b2d73697a6520646174612e20546865206b6579206e6565647320746f20626520686173686564206265666f7265206265696e6720757365642062792074686520484d414320616c676f726974686d2e",
            sha256: "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
            sha384: "6617178e941f020d351e2f254e8fd32c602420feb0b8fb9adccebb82461e99c5a678cc31e799176d3860e6110c46523e",
            sha512: "e37b6a775dc87dbaa4dfa9f96e5e3ffddebd71f8867289865df5a32d20cdc944b6022cac3c4982b10d5eeb55c3e4de15134676fb6de0446065c97440fa8c6a58",
        },
    ];

    #[test]
    fn rfc_4231_vectors() {
        for (i, case) in RFC_4231_CASES.iter().enumerate() {
            let key = hex_decode(case.key);
            let data = hex_decode(case.data);
            assert_eq!(hex(&Hmac::<Sha256>::mac(&key, &data)), case.sha256, "case {i} sha256");
            assert_eq!(hex(&Hmac::<Sha384>::mac(&key, &data)), case.sha384, "case {i} sha384");
            assert_eq!(hex(&Hmac::<Sha512>::mac(&key, &data)), case.sha512, "case {i} sha512");
        }
    }

    #[test]
    fn streaming_matches_one_shot() {
        let key = b"key material that happens to be longer than one block, forcing HMAC to hash it down first before use as the padded key";
        let data: Vec<u8> = (0..500u32).map(|i| (i % 256) as u8).collect();
        let expected = Hmac::<Sha256>::mac(key, &data);
        for split in [0, 1, 63, 64, 65, 200, data.len()] {
            let mut h = Hmac::<Sha256>::new(key);
            h.update(&data[..split]);
            h.update(&data[split..]);
            assert_eq!(h.finalize(), expected);
        }
    }

    #[test]
    fn verify_accepts_correct_and_rejects_tampered() {
        let key = b"a key";
        let data = b"some data";
        let tag = Hmac::<Sha256>::mac(key, data);
        assert!(Hmac::<Sha256>::verify(key, data, tag.as_bytes()));

        let mut bad_tag = *tag.as_bytes().first_chunk::<32>().unwrap();
        bad_tag[0] ^= 1;
        assert!(!Hmac::<Sha256>::verify(key, data, &bad_tag));

        assert!(!Hmac::<Sha256>::verify(key, b"other data", tag.as_bytes()));
        assert!(!Hmac::<Sha256>::verify(b"other key", data, tag.as_bytes()));
    }

    #[test]
    fn verify_accepts_truncated_tag_and_rejects_wrong_truncation() {
        let key = b"a key";
        let data = b"some data";
        let tag = Hmac::<Sha256>::mac(key, data);
        assert!(Hmac::<Sha256>::verify(key, data, &tag.as_bytes()[..16]));
        let mut truncated = [0u8; 16];
        truncated.copy_from_slice(&tag.as_bytes()[..16]);
        truncated[15] ^= 0xff;
        assert!(!Hmac::<Sha256>::verify(key, data, &truncated));
        // A tag longer than the hash output can never verify.
        let mut too_long = alloc::vec![0u8; 33];
        too_long[..32].copy_from_slice(tag.as_bytes());
        assert!(!Hmac::<Sha256>::verify(key, data, &too_long));
    }
}
