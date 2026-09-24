//! RSASSA-PKCS1-v1_5 and RSASSA-PSS signature *verification* (RFC 8017
//! sections 8.2.2 and 8.1.2), the two RSA signature schemes TLS 1.3
//! certificate chains and `CertificateVerify` messages use. Verification only
//! -- this crate never holds an RSA private key -- so every value here is
//! public; see [`crate::bigint`]'s module doc for why that means no
//! constant-time requirement applies.
//!
//! [`PublicKey::verify_pkcs1v15`] rebuilds the *entire* expected
//! `EMSA-PKCS1-v1_5` encoding (leading `0x00 0x01`, the `0xff` padding run,
//! the DigestInfo DER prefix, the digest) and compares it byte-for-byte
//! against the decrypted signature, rather than scanning through the padding
//! looking for a `0x00` separator -- the "never parse padding loosely"
//! design the brief calls for, and the standard defense against
//! Bleichenbacher-style low-exponent forgeries that loose parsers have
//! historically fallen for.
//!
//! [`PublicKey::verify_pss`] implements `EMSA-PSS-VERIFY` (RFC 8017 section
//! 9.1.2) in full, including the "top bits of the leftmost octet must be
//! zero" check (relevant whenever the modulus's bit length is not a multiple
//! of 8 -- never true for the 2048/3072/4096-bit keys this crate accepts, but
//! implemented generally rather than assumed away) and the salt-length
//! convention TLS 1.3 requires: salt length equal to the hash's output length.

use crate::bigint::{BigUint, Modulus};
use crate::hash::Digest;
use crate::sha256::Sha256;
use crate::sha512::{Sha384, Sha512};

/// The largest modulus this crate accepts, in bytes (4096 bits).
const MAX_MODULUS_BYTES: usize = 512;

/// A hash algorithm RSA signature verification can use: which digest
/// [`PublicKey::verify_pkcs1v15`]/[`PublicKey::verify_pss`] expect `digest` to
/// already be the output of, and which the DigestInfo OID (PKCS1-v1_5) or
/// MGF1 (PSS) is built from internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlg {
    /// SHA-256.
    Sha256,
    /// SHA-384.
    Sha384,
    /// SHA-512.
    Sha512,
}

// RFC 8017's DigestInfo DER prefix for each hash: `SEQUENCE { SEQUENCE { OID
// <hash>, NULL }, OCTET STRING <digest> }` up to (not including) the digest
// bytes themselves -- `0x30 0x06 0x09 60 86 48 01 65 03 04 02 0N 05 00 04 <hLen>`
// give or take the two length bytes at the front, which depend on `hLen`.
// These exact byte strings are the same ones reproduced in essentially every
// RSA PKCS#1 v1.5 implementation (e.g. Go's `crypto/rsa`); derived here from
// first principles (the DER encoding of OID 2.16.840.1.101.3.4.2.{1,2,3}) and
// cross-checked against that widely-published table, not copied from a crate
// (D2/D22: no crypto crates of any kind).
const SHA256_PREFIX: [u8; 19] =
    [0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];
const SHA384_PREFIX: [u8; 19] =
    [0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30];
const SHA512_PREFIX: [u8; 19] =
    [0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40];

impl HashAlg {
    /// The digest length in bytes (32/48/64).
    pub const fn output_len(self) -> usize {
        match self {
            HashAlg::Sha256 => 32,
            HashAlg::Sha384 => 48,
            HashAlg::Sha512 => 64,
        }
    }

    fn hash(self, data: &[u8]) -> Digest {
        match self {
            HashAlg::Sha256 => Digest::from_slice(&Sha256::digest(data)),
            HashAlg::Sha384 => Digest::from_slice(&Sha384::digest(data)),
            HashAlg::Sha512 => Digest::from_slice(&Sha512::digest(data)),
        }
    }

    fn digest_info_prefix(self) -> &'static [u8] {
        match self {
            HashAlg::Sha256 => &SHA256_PREFIX,
            HashAlg::Sha384 => &SHA384_PREFIX,
            HashAlg::Sha512 => &SHA512_PREFIX,
        }
    }
}

/// An RSA verification failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsaError {
    /// `from_components`: the modulus was not 2048-4096 bits, the exponent
    /// was even or less than 3, or a value did not fit this crate's
    /// [`crate::bigint`] capacity.
    InvalidKey,
    /// The signature's length did not match the modulus's, or its integer
    /// value was not in `[0, n)` (RFC 8017's "signature representative out of
    /// range").
    InvalidSignature,
    /// `digest`'s length did not match `alg`'s output size.
    InvalidDigestLength,
    /// The signature decrypted, but its padding/hash did not match: not a
    /// valid signature over this digest under this key.
    VerificationFailed,
}

/// An RSA public key: a modulus and public exponent, ready to verify
/// PKCS1-v1_5 or PSS signatures against.
pub struct PublicKey {
    modulus: Modulus,
    e: BigUint,
    /// The modulus's length in bytes (`k` in RFC 8017), i.e. every signature
    /// this key accepts must be exactly this many bytes.
    k: usize,
}

impl PublicKey {
    /// Builds a public key from a big-endian modulus and exponent (as they
    /// appear in a certificate's SubjectPublicKeyInfo). Rejects moduli
    /// outside 2048-4096 bits and exponents that are even or less than 3
    /// (brief M8-T3's key-validity requirement).
    pub fn from_components(n: &[u8], e: &[u8]) -> Result<PublicKey, RsaError> {
        let n_big = BigUint::from_be_bytes(n).map_err(|_| RsaError::InvalidKey)?;
        let bits = n_big.bit_len();
        if !(2048..=4096).contains(&bits) {
            return Err(RsaError::InvalidKey);
        }
        let e_big = BigUint::from_be_bytes(e).map_err(|_| RsaError::InvalidKey)?;
        if e_big < BigUint::from_u64(3) || !e_big.bit(0) {
            return Err(RsaError::InvalidKey);
        }
        let k = bits.div_ceil(8);
        let modulus = Modulus::new(n_big).map_err(|_| RsaError::InvalidKey)?;
        Ok(PublicKey { modulus, e: e_big, k })
    }

    /// `s^e mod n`, after checking `s` (the signature, big-endian, exactly
    /// `self.k` bytes) is in range. Shared by both verification schemes.
    fn rsavp1(&self, sig: &[u8]) -> Result<BigUint, RsaError> {
        if sig.len() != self.k {
            return Err(RsaError::InvalidSignature);
        }
        let s = BigUint::from_be_bytes(sig).map_err(|_| RsaError::InvalidSignature)?;
        if s >= *self.modulus.value() {
            return Err(RsaError::InvalidSignature);
        }
        Ok(self.modulus.pow_mod(&s, &self.e))
    }

    /// RSASSA-PKCS1-V1_5-VERIFY (RFC 8017 section 8.2.2): `digest` is the
    /// caller's already-computed `alg` digest of the signed data (a TLS 1.3
    /// certificate's TBSCertificate, or a CertificateVerify's context string
    /// plus transcript hash); `sig` is the raw (not DER-wrapped) signature.
    pub fn verify_pkcs1v15(&self, alg: HashAlg, digest: &Digest, sig: &[u8]) -> Result<(), RsaError> {
        if digest.as_bytes().len() != alg.output_len() {
            return Err(RsaError::InvalidDigestLength);
        }
        let m = self.rsavp1(sig)?;
        let mut em_actual = [0u8; MAX_MODULUS_BYTES];
        m.to_be_bytes(&mut em_actual[..self.k]).map_err(|_| RsaError::VerificationFailed)?;

        let prefix = alg.digest_info_prefix();
        let d = digest.as_bytes();
        let t_len = prefix.len() + d.len();
        if self.k < t_len + 11 {
            // k must be >= tLen + 11 for at least 8 bytes of 0xff padding
            // (RFC 8017 section 9.2); never true for real 2048+-bit keys.
            return Err(RsaError::VerificationFailed);
        }
        let ps_len = self.k - t_len - 3;

        let mut em_expected = [0u8; MAX_MODULUS_BYTES];
        em_expected[0] = 0x00;
        em_expected[1] = 0x01;
        for b in em_expected[2..2 + ps_len].iter_mut() {
            *b = 0xff;
        }
        em_expected[2 + ps_len] = 0x00;
        em_expected[3 + ps_len..3 + ps_len + prefix.len()].copy_from_slice(prefix);
        em_expected[3 + ps_len + prefix.len()..self.k].copy_from_slice(d);

        if em_actual[..self.k] == em_expected[..self.k] {
            Ok(())
        } else {
            Err(RsaError::VerificationFailed)
        }
    }

    /// RSASSA-PSS-VERIFY (RFC 8017 section 8.1.2 + `EMSA-PSS-VERIFY`, section
    /// 9.1.2), MGF1 built on `alg` itself, salt length fixed to `alg`'s
    /// output length (TLS 1.3's requirement, RFC 8446 section 4.2.3).
    pub fn verify_pss(&self, alg: HashAlg, digest: &Digest, sig: &[u8]) -> Result<(), RsaError> {
        if digest.as_bytes().len() != alg.output_len() {
            return Err(RsaError::InvalidDigestLength);
        }
        let m = self.rsavp1(sig)?;

        let mod_bits = self.modulus.value().bit_len();
        let em_bits = mod_bits - 1;
        let em_len = em_bits.div_ceil(8);
        let mut em_buf = [0u8; MAX_MODULUS_BYTES];
        if m.to_be_bytes(&mut em_buf[..em_len]).is_err() {
            return Err(RsaError::VerificationFailed);
        }
        let em = &em_buf[..em_len];

        let h_len = alg.output_len();
        let s_len = h_len; // Brief/TLS 1.3: salt length == hash length.
        if em_len < h_len + s_len + 2 || em[em_len - 1] != 0xbc {
            return Err(RsaError::VerificationFailed);
        }

        let db_len = em_len - h_len - 1;
        let (masked_db, rest) = em.split_at(db_len);
        let h = &rest[..h_len];

        // RFC 8017 step 6: the top `8*emLen - emBits` bits of the leftmost
        // octet of maskedDB must be zero (only ever nonzero here when
        // `mod_bits` is not a multiple of 8, which none of this crate's
        // accepted key sizes are, but checked unconditionally per spec).
        let top_zero_bits = 8 * em_len - em_bits;
        let top_mask = if top_zero_bits > 0 { 0xffu8 << (8 - top_zero_bits) } else { 0 };
        if masked_db[0] & top_mask != 0 {
            return Err(RsaError::VerificationFailed);
        }

        let mut db = [0u8; MAX_MODULUS_BYTES];
        mgf1(alg, h, &mut db[..db_len]);
        for i in 0..db_len {
            db[i] ^= masked_db[i];
        }
        db[0] &= !top_mask;

        let zero_len = db_len - s_len - 1;
        if db[..zero_len].iter().any(|&b| b != 0) || db[zero_len] != 0x01 {
            return Err(RsaError::VerificationFailed);
        }
        let salt = &db[zero_len + 1..db_len];

        // M' = eight zero octets || mHash || salt (RFC 8017 step 12).
        let mut m_prime = [0u8; 8 + 64 + 64];
        m_prime[8..8 + d_len(digest)].copy_from_slice(digest.as_bytes());
        m_prime[8 + d_len(digest)..8 + d_len(digest) + salt.len()].copy_from_slice(salt);
        let h_prime = alg.hash(&m_prime[..8 + d_len(digest) + salt.len()]);

        if h_prime.as_bytes() == h {
            Ok(())
        } else {
            Err(RsaError::VerificationFailed)
        }
    }
}

fn d_len(digest: &Digest) -> usize {
    digest.as_bytes().len()
}

/// MGF1 (RFC 8017 Appendix B.2.1): fills `out` with `Hash(seed || 0)`,
/// `Hash(seed || 1)`, ... concatenated and truncated to `out.len()`, the
/// 4-byte counters big-endian.
fn mgf1(alg: HashAlg, seed: &[u8], out: &mut [u8]) {
    let h_len = alg.output_len();
    let mut block_input = [0u8; 64 + 4];
    block_input[..seed.len()].copy_from_slice(seed);

    let mut written = 0;
    let mut counter: u32 = 0;
    while written < out.len() {
        block_input[seed.len()..seed.len() + 4].copy_from_slice(&counter.to_be_bytes());
        let block = alg.hash(&block_input[..seed.len() + 4]);
        let take = core::cmp::min(h_len, out.len() - written);
        out[written..written + take].copy_from_slice(&block.as_bytes()[..take]);
        written += take;
        counter += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_info_prefixes_have_the_expected_shape() {
        for (alg, prefix) in [(HashAlg::Sha256, &SHA256_PREFIX[..]), (HashAlg::Sha384, &SHA384_PREFIX[..]), (HashAlg::Sha512, &SHA512_PREFIX[..])]
        {
            // SEQUENCE tag, total length = 17 + hLen, ending in an OCTET
            // STRING tag/length matching the hash's own output size.
            assert_eq!(prefix[0], 0x30);
            assert_eq!(prefix[1] as usize, 17 + alg.output_len());
            assert_eq!(prefix[prefix.len() - 2], 0x04);
            assert_eq!(prefix[prefix.len() - 1] as usize, alg.output_len());
        }
    }

    #[test]
    fn mgf1_first_block_matches_direct_hash_with_zero_counter() {
        let seed = [0xabu8; 32];
        let mut out = [0u8; 32];
        mgf1(HashAlg::Sha256, &seed, &mut out);

        let mut expected_input = [0u8; 36];
        expected_input[..32].copy_from_slice(&seed);
        let expected = Sha256::digest(&expected_input);
        assert_eq!(&out[..], &expected[..]);
    }

    #[test]
    fn mgf1_output_is_deterministic_and_spans_multiple_blocks() {
        let seed = [0x11u8; 48];
        let mut a = [0u8; 100]; // more than 2 SHA-256 blocks (2*32=64).
        let mut b = [0u8; 100];
        mgf1(HashAlg::Sha256, &seed, &mut a);
        mgf1(HashAlg::Sha256, &seed, &mut b);
        assert_eq!(a, b);
        assert!(a[..32] != a[32..64], "first two blocks should differ (different counters)");
    }

    #[test]
    fn from_components_rejects_out_of_range_modulus() {
        let small_n = [0xffu8; 128]; // 1024 bits, below the 2048 minimum.
        assert_eq!(PublicKey::from_components(&small_n, &[0x01, 0x00, 0x01]).err(), Some(RsaError::InvalidKey));
    }

    #[test]
    fn from_components_rejects_even_exponent() {
        let mut n = [0u8; 256];
        n[0] = 0x80; // exactly 2048 bits.
        assert_eq!(PublicKey::from_components(&n, &[0x02]).err(), Some(RsaError::InvalidKey));
    }

    #[test]
    fn from_components_rejects_exponent_below_three() {
        let mut n = [0u8; 256];
        n[0] = 0x80;
        assert_eq!(PublicKey::from_components(&n, &[0x01]).err(), Some(RsaError::InvalidKey));
    }

    #[test]
    fn verify_rejects_wrong_length_signature() {
        let mut n = [0u8; 256];
        n[0] = 0x80;
        n[255] |= 1; // keep it odd.
        let key = PublicKey::from_components(&n, &[0x01, 0x00, 0x01]).unwrap();
        let digest = Digest::from_slice(&Sha256::digest(b"hello"));
        let short_sig = [0u8; 255];
        assert_eq!(key.verify_pkcs1v15(HashAlg::Sha256, &digest, &short_sig), Err(RsaError::InvalidSignature));
    }

    #[test]
    fn verify_rejects_mismatched_digest_length() {
        let mut n = [0u8; 256];
        n[0] = 0x80;
        n[255] |= 1;
        let key = PublicKey::from_components(&n, &[0x01, 0x00, 0x01]).unwrap();
        let digest = Digest::from_slice(&Sha256::digest(b"hello")); // 32 bytes, not SHA-384's 48.
        let sig = [0u8; 256];
        assert_eq!(key.verify_pkcs1v15(HashAlg::Sha384, &digest, &sig), Err(RsaError::InvalidDigestLength));
    }
}
