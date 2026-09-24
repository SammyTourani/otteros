//! `AlgorithmIdentifier` (RFC 5280 section 4.1.1.2) for a certificate
//! signature: which `otter-crypto` hash and verification scheme it names,
//! including RSASSA-PSS's own explicit parameters (RFC 8017 appendix A.2.3).
//! SHA-1 and MD5 signature algorithms are recognized only so they can be
//! rejected by name (DECISIONS.md D22).
//!
//! [`verify_signature`] is the one place this crate calls into
//! `otter-crypto`'s signature verification: it picks the hash function from
//! [`SignatureAlgorithm`], hashes the signed bytes, and dispatches to
//! whichever of RSA PKCS#1-v1.5, RSASSA-PSS or ECDSA P-256/P-384 matches
//! both the algorithm identifier and the issuer's actual key type -- a
//! mismatch between the two (e.g. an `ecdsa-with-SHA256` signature claimed
//! under a P-384 key) is rejected rather than guessed at.

use otter_crypto::hash::Digest;
use otter_crypto::rsa::HashAlg;
use otter_crypto::sha256::Sha256;
use otter_crypto::sha512::{Sha384, Sha512};

use crate::der::{DerError, Reader};
use crate::error::X509Error;
use crate::oid;
use crate::spki::PublicKey;

/// A certificate (or `CertificateVerify`) signature's scheme and hash,
/// decoded from its `AlgorithmIdentifier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    /// `sha{256,384,512}WithRSAEncryption`.
    RsaPkcs1(HashAlg),
    /// `id-RSASSA-PSS`, with parameters already checked to be this crate's
    /// one supported configuration (MGF1 with the same hash, salt length
    /// equal to the hash's output length, default trailer field).
    RsaPss(HashAlg),
    /// `ecdsa-with-SHA256`, only valid paired with a P-256 key.
    EcdsaSha256,
    /// `ecdsa-with-SHA384`, only valid paired with a P-384 key.
    EcdsaSha384,
}

fn require_empty(r: &Reader<'_>) -> Result<(), DerError> {
    if r.is_empty() { Ok(()) } else { Err(DerError::TrailingData) }
}

fn oid_to_hash_alg(oid_bytes: &[u8]) -> Result<HashAlg, X509Error> {
    if oid_bytes == oid::ID_SHA256 {
        Ok(HashAlg::Sha256)
    } else if oid_bytes == oid::ID_SHA384 {
        Ok(HashAlg::Sha384)
    } else if oid_bytes == oid::ID_SHA512 {
        Ok(HashAlg::Sha512)
    } else {
        Err(X509Error::UnsupportedSignatureAlgorithm)
    }
}

/// Reads a `HashAlgorithm` (a plain `AlgorithmIdentifier` naming a hash),
/// tolerating an absent or present-and-`NULL` parameters field (both appear
/// in real certificates).
fn parse_hash_algorithm_identifier(r: &mut Reader<'_>) -> Result<HashAlg, X509Error> {
    let mut alg = r.read_sequence()?;
    let hash = oid_to_hash_alg(alg.read_oid()?)?;
    if !alg.is_empty() {
        alg.read_null()?;
    }
    require_empty(&alg)?;
    Ok(hash)
}

/// Parses `RSASSA-PSS-params` (RFC 8017 appendix A.2.3), accepting only this
/// crate's one supported configuration: an explicit (non-default) SHA-256,
/// SHA-384 or SHA-512 hash; MGF1 using that same hash; a salt length equal
/// to the hash's output length; and, if present, a trailer field of `1`.
/// Every `DEFAULT` in the ASN.1 (SHA-1-based) is a scheme this crate does
/// not support, so an absent field -- not just a wrong one -- is rejected.
fn parse_pss_params(mut r: Reader<'_>) -> Result<HashAlg, X509Error> {
    let hash_alg = match r.read_explicit(0)? {
        None => return Err(X509Error::UnsupportedSignatureAlgorithm),
        Some(mut inner) => {
            let hash = parse_hash_algorithm_identifier(&mut inner)?;
            require_empty(&inner)?;
            hash
        }
    };

    let mgf_hash = match r.read_explicit(1)? {
        None => return Err(X509Error::UnsupportedSignatureAlgorithm),
        Some(mut inner) => {
            let mut alg = inner.read_sequence()?;
            let mgf_oid = alg.read_oid()?;
            if mgf_oid != oid::MGF1 {
                return Err(X509Error::UnsupportedSignatureAlgorithm);
            }
            // MGF1's own parameters are a nested HashAlgorithm AlgorithmIdentifier.
            let mgf_hash = parse_hash_algorithm_identifier(&mut alg)?;
            require_empty(&alg)?;
            require_empty(&inner)?;
            mgf_hash
        }
    };
    if mgf_hash != hash_alg {
        return Err(X509Error::UnsupportedSignatureAlgorithm);
    }

    let salt_len = match r.read_explicit(2)? {
        None => return Err(X509Error::UnsupportedSignatureAlgorithm),
        Some(mut inner) => {
            let len = inner.read_small_uint()?;
            require_empty(&inner)?;
            len
        }
    };
    if salt_len as usize != hash_alg.output_len() {
        return Err(X509Error::UnsupportedSignatureAlgorithm);
    }

    if let Some(mut inner) = r.read_explicit(3)? {
        let trailer = inner.read_small_uint()?;
        require_empty(&inner)?;
        if trailer != 1 {
            return Err(X509Error::UnsupportedSignatureAlgorithm);
        }
    }
    require_empty(&r)?;
    Ok(hash_alg)
}

/// Parses a signature `AlgorithmIdentifier`'s content bytes (the bytes
/// inside its `SEQUENCE`, as e.g. [`crate::der::Reader::read_tlv`] returns).
pub fn parse_algorithm_identifier(content: &[u8]) -> Result<SignatureAlgorithm, X509Error> {
    let mut r = Reader::new(content);
    let alg_oid = r.read_oid()?;

    let result = if alg_oid == oid::SHA256_WITH_RSA_ENCRYPTION {
        r.read_null()?;
        SignatureAlgorithm::RsaPkcs1(HashAlg::Sha256)
    } else if alg_oid == oid::SHA384_WITH_RSA_ENCRYPTION {
        r.read_null()?;
        SignatureAlgorithm::RsaPkcs1(HashAlg::Sha384)
    } else if alg_oid == oid::SHA512_WITH_RSA_ENCRYPTION {
        r.read_null()?;
        SignatureAlgorithm::RsaPkcs1(HashAlg::Sha512)
    } else if alg_oid == oid::ECDSA_WITH_SHA256 {
        SignatureAlgorithm::EcdsaSha256
    } else if alg_oid == oid::ECDSA_WITH_SHA384 {
        SignatureAlgorithm::EcdsaSha384
    } else if alg_oid == oid::RSASSA_PSS {
        let params = r.read_sequence()?;
        SignatureAlgorithm::RsaPss(parse_pss_params(params)?)
    } else {
        // Covers sha1WithRSAEncryption, md5WithRSAEncryption,
        // ecdsa-with-SHA1 and anything else this crate does not implement.
        return Err(X509Error::UnsupportedSignatureAlgorithm);
    };
    require_empty(&r)?;
    Ok(result)
}

/// The hash this algorithm signs over, applied to `data`.
fn digest_for(alg: SignatureAlgorithm, data: &[u8]) -> Digest {
    let hash_alg = match alg {
        SignatureAlgorithm::RsaPkcs1(h) | SignatureAlgorithm::RsaPss(h) => h,
        SignatureAlgorithm::EcdsaSha256 => HashAlg::Sha256,
        SignatureAlgorithm::EcdsaSha384 => HashAlg::Sha384,
    };
    match hash_alg {
        HashAlg::Sha256 => Digest::from_slice(&Sha256::digest(data)),
        HashAlg::Sha384 => Digest::from_slice(&Sha384::digest(data)),
        HashAlg::Sha512 => Digest::from_slice(&Sha512::digest(data)),
    }
}

/// Verifies that `signature` (raw `signatureValue` BIT STRING content) is a
/// valid `alg` signature over `signed_data` under `issuer_key`. Rejects any
/// combination of key type and algorithm this crate does not implement
/// (including a technically-possible but never-issued-in-practice pairing
/// like a P-384 key with `ecdsa-with-SHA256`) the same way it rejects an
/// invalid signature: both cases mean "cannot verify this", which is exactly
/// [`X509Error::SignatureVerificationFailed`]/[`X509Error::UnsupportedSignatureAlgorithm`]'s
/// shared job of aborting the chain.
pub fn verify_signature(
    issuer_key: &PublicKey,
    alg: SignatureAlgorithm,
    signed_data: &[u8],
    signature: &[u8],
) -> Result<(), X509Error> {
    let digest = digest_for(alg, signed_data);
    match (issuer_key, alg) {
        (PublicKey::Rsa(key), SignatureAlgorithm::RsaPkcs1(h)) => {
            key.verify_pkcs1v15(h, &digest, signature).map_err(|_| X509Error::SignatureVerificationFailed)
        }
        (PublicKey::Rsa(key), SignatureAlgorithm::RsaPss(h)) => {
            key.verify_pss(h, &digest, signature).map_err(|_| X509Error::SignatureVerificationFailed)
        }
        (PublicKey::P256(key), SignatureAlgorithm::EcdsaSha256) => {
            key.verify(digest.as_bytes(), signature).map_err(|_| X509Error::SignatureVerificationFailed)
        }
        (PublicKey::P384(key), SignatureAlgorithm::EcdsaSha384) => {
            key.verify(digest.as_bytes(), signature).map_err(|_| X509Error::SignatureVerificationFailed)
        }
        _ => Err(X509Error::UnsupportedSignatureAlgorithm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `AlgorithmIdentifier`'s *content* bytes (what
    /// [`parse_algorithm_identifier`] takes: the `SEQUENCE` tag/length are
    /// stripped by the caller, e.g. [`crate::cert::Certificate::parse`],
    /// before this function ever sees them).
    fn oid_seq(oid_bytes: &[u8], extra: &[u8]) -> alloc::vec::Vec<u8> {
        let mut content = alloc::vec![0x06u8, oid_bytes.len() as u8];
        content.extend_from_slice(oid_bytes);
        content.extend_from_slice(extra);
        content
    }

    #[test]
    fn recognizes_sha256_with_rsa() {
        let der = oid_seq(oid::SHA256_WITH_RSA_ENCRYPTION, &[0x05, 0x00]);
        assert_eq!(parse_algorithm_identifier(&der).unwrap(), SignatureAlgorithm::RsaPkcs1(HashAlg::Sha256));
    }

    #[test]
    fn recognizes_ecdsa_with_sha384_with_no_parameters() {
        let der = oid_seq(oid::ECDSA_WITH_SHA384, &[]);
        assert_eq!(parse_algorithm_identifier(&der).unwrap(), SignatureAlgorithm::EcdsaSha384);
    }

    #[test]
    fn rejects_sha1_with_rsa() {
        let der = oid_seq(oid::SHA1_WITH_RSA_ENCRYPTION, &[0x05, 0x00]);
        assert_eq!(parse_algorithm_identifier(&der).err(), Some(X509Error::UnsupportedSignatureAlgorithm));
    }

    #[test]
    fn rejects_md5_with_rsa() {
        let der = oid_seq(oid::MD5_WITH_RSA_ENCRYPTION, &[0x05, 0x00]);
        assert_eq!(parse_algorithm_identifier(&der).err(), Some(X509Error::UnsupportedSignatureAlgorithm));
    }

    #[test]
    fn rejects_ecdsa_with_sha1() {
        let der = oid_seq(oid::ECDSA_WITH_SHA1, &[]);
        assert_eq!(parse_algorithm_identifier(&der).err(), Some(X509Error::UnsupportedSignatureAlgorithm));
    }

    #[test]
    fn rejects_ecdsa_with_unexpected_trailing_parameters() {
        let der = oid_seq(oid::ECDSA_WITH_SHA256, &[0x05, 0x00]); // NULL params not allowed here
        assert!(parse_algorithm_identifier(&der).is_err());
    }

    /// Builds a full RSASSA-PSS `AlgorithmIdentifier` naming `hash` for both
    /// the hash and MGF1, with `salt_len` and no trailer field.
    fn pss_der(hash_oid: &[u8], salt_len: u8) -> alloc::vec::Vec<u8> {
        let mut hash_alg = alloc::vec![0x06u8, hash_oid.len() as u8];
        hash_alg.extend_from_slice(hash_oid);
        let hash_alg_seq = wrap_seq(&hash_alg);

        let explicit0 = wrap_explicit(0, &hash_alg_seq);

        let mut mgf_content = alloc::vec![0x06u8, oid::MGF1.len() as u8];
        mgf_content.extend_from_slice(oid::MGF1);
        mgf_content.extend_from_slice(&hash_alg_seq);
        let mgf_seq = wrap_seq(&mgf_content);
        let explicit1 = wrap_explicit(1, &mgf_seq);

        let salt_int = alloc::vec![0x02u8, 0x01, salt_len];
        let explicit2 = wrap_explicit(2, &salt_int);

        let mut params_content = explicit0;
        params_content.extend(explicit1);
        params_content.extend(explicit2);
        let params_seq = wrap_seq(&params_content);

        let mut alg_content = alloc::vec![0x06u8, oid::RSASSA_PSS.len() as u8];
        alg_content.extend_from_slice(oid::RSASSA_PSS);
        alg_content.extend(params_seq);
        alg_content
    }

    fn wrap_seq(content: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec![0x30u8, content.len() as u8];
        out.extend_from_slice(content);
        out
    }

    fn wrap_explicit(n: u8, content: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec![0xa0u8 | n, content.len() as u8];
        out.extend_from_slice(content);
        out
    }

    #[test]
    fn pss_with_matching_salt_length_is_accepted() {
        let der = pss_der(oid::ID_SHA256, 32);
        assert_eq!(parse_algorithm_identifier(&der).unwrap(), SignatureAlgorithm::RsaPss(HashAlg::Sha256));
    }

    #[test]
    fn pss_with_mismatched_salt_length_is_rejected() {
        let der = pss_der(oid::ID_SHA256, 20); // sha1-style salt length, not 32
        assert_eq!(parse_algorithm_identifier(&der).err(), Some(X509Error::UnsupportedSignatureAlgorithm));
    }

    #[test]
    fn pss_sha512_with_matching_salt_length_is_accepted() {
        let der = pss_der(oid::ID_SHA512, 64);
        assert_eq!(parse_algorithm_identifier(&der).unwrap(), SignatureAlgorithm::RsaPss(HashAlg::Sha512));
    }
}
