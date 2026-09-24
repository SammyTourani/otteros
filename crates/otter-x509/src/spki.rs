//! `SubjectPublicKeyInfo` (RFC 5280 section 4.1.2.7): the certificate's own
//! public key, decoded into an `otter-crypto` key ready to verify a
//! signature against -- either the next certificate's signature, while
//! walking a certification path, or (via [`crate::VerifiedChain`]) a TLS
//! 1.3 `CertificateVerify`. Only `rsaEncryption` and `id-ecPublicKey` with
//! the `prime256v1`/`secp384r1` named curves are supported (brief M8-T4's
//! scope); anything else -- DSA, X25519/Ed25519 keys, EC keys with explicit
//! (rather than named) domain parameters or an unrecognized curve --
//! is [`X509Error::UnsupportedPublicKey`].

use alloc::boxed::Box;

use crate::der::{DerError, Reader};
use crate::error::X509Error;
use crate::oid;

/// A certificate's public key, decoded and ready for `otter-crypto` to
/// verify a signature against. Owns its data (no lifetime): safe to carry
/// in a [`crate::VerifiedChain`] returned from [`crate::verify_server_chain`]
/// well after the certificate's own DER bytes have gone out of scope.
///
/// The RSA variant is boxed: an `otter_crypto::rsa::PublicKey` (a 4096-bit-
/// capable `Modulus` plus exponent) is over 2 KiB, more than twice the size
/// of the largest EC variant, and every `Certificate` (brief M8-T4 parses
/// one per certificate in a chain, plus every embedded root) carries one --
/// boxing keeps `size_of::<PublicKey>()` from ballooning to the RSA case's
/// size regardless of which variant is actually in use.
pub enum PublicKey {
    /// An RSA key (2048-4096 bits; PKCS#1-v1.5 or PSS signatures).
    Rsa(Box<otter_crypto::rsa::PublicKey>),
    /// A NIST P-256 (secp256r1) key.
    P256(otter_crypto::p256::PublicKey),
    /// A NIST P-384 (secp384r1) key.
    P384(otter_crypto::p384::PublicKey),
}

/// Reads a `SubjectPublicKeyInfo` `SEQUENCE` from `r`.
pub fn parse_spki<'a>(r: &mut Reader<'a>) -> Result<PublicKey, X509Error> {
    let mut spki = r.read_sequence()?;
    let mut alg = spki.read_sequence()?;
    let alg_oid = alg.read_oid()?;

    let key = if alg_oid == oid::RSA_ENCRYPTION {
        alg.read_null()?;
        require_empty(&alg)?;
        let bits = spki.read_bit_string_bytes()?;
        let mut rsa_key = Reader::new(bits);
        let mut rsa_seq = rsa_key.read_sequence()?;
        let n = rsa_seq.read_uint_bytes()?;
        let e = rsa_seq.read_uint_bytes()?;
        require_empty(&rsa_seq)?;
        require_empty(&rsa_key)?;
        let pk = otter_crypto::rsa::PublicKey::from_components(n, e).map_err(|_| X509Error::InvalidPublicKey)?;
        PublicKey::Rsa(Box::new(pk))
    } else if alg_oid == oid::EC_PUBLIC_KEY {
        let curve_oid = alg.read_oid()?;
        require_empty(&alg)?;
        let point = spki.read_bit_string_bytes()?;
        if curve_oid == oid::PRIME256V1 {
            let pk = otter_crypto::p256::PublicKey::from_sec1(point).map_err(|_| X509Error::InvalidPublicKey)?;
            PublicKey::P256(pk)
        } else if curve_oid == oid::SECP384R1 {
            let pk = otter_crypto::p384::PublicKey::from_sec1(point).map_err(|_| X509Error::InvalidPublicKey)?;
            PublicKey::P384(pk)
        } else {
            return Err(X509Error::UnsupportedPublicKey);
        }
    } else {
        return Err(X509Error::UnsupportedPublicKey);
    };

    require_empty(&spki)?;
    Ok(key)
}

fn require_empty(r: &Reader<'_>) -> Result<(), DerError> {
    if r.is_empty() { Ok(()) } else { Err(DerError::TrailingData) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap_seq(content: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec![0x30u8, content.len() as u8];
        out.extend_from_slice(content);
        out
    }

    #[test]
    fn rejects_unknown_algorithm_oid() {
        // DSA, 1.2.840.10040.4.1 -- not supported.
        let oid_bytes = [0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x38, 0x04, 0x01];
        let mut alg = oid_bytes.to_vec();
        alg.extend([0x05, 0x00]); // NULL params
        let alg_seq = wrap_seq(&alg);
        let mut spki_content = alg_seq;
        spki_content.extend([0x03, 0x02, 0x00, 0xff]); // dummy BIT STRING
        let der = wrap_seq(&spki_content);
        let mut r = Reader::new(&der);
        assert_eq!(parse_spki(&mut r).err(), Some(X509Error::UnsupportedPublicKey));
    }

    #[test]
    fn rejects_ec_key_with_unknown_curve() {
        // id-ecPublicKey with secp256k1 (1.3.132.0.10), not one of the two supported curves.
        let mut alg = oid::EC_PUBLIC_KEY.to_vec();
        let mut alg_der = alloc::vec![0x06u8, alg.len() as u8];
        alg_der.append(&mut alg);
        alg_der.extend([0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x0a]);
        let alg_seq = wrap_seq(&alg_der);
        let mut spki_content = alg_seq;
        spki_content.extend([0x03, 0x02, 0x00, 0xff]);
        let der = wrap_seq(&spki_content);
        let mut r = Reader::new(&der);
        assert_eq!(parse_spki(&mut r).err(), Some(X509Error::UnsupportedPublicKey));
    }
}
