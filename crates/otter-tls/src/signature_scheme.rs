//! `SignatureScheme` (RFC 8446 section 4.2.3): the codepoints this client
//! advertises in `signature_algorithms`/`signature_algorithms_cert` and uses
//! to verify a `CertificateVerify` message. Certificate-signature schemes
//! (RFC 5280 `AlgorithmIdentifier`s) are `otter_x509`'s job, not this
//! module's -- `otter_x509::verify_server_chain` already checked every
//! signature *inside* the chain; this module only verifies the live
//! `CertificateVerify` signature the server computes with its leaf
//! certificate's private key over this connection's own transcript.

use alloc::vec::Vec;

use otter_crypto::hash::Digest;
use otter_crypto::rsa::HashAlg;
use otter_crypto::{Sha256, Sha384, Sha512};

use crate::error::TlsError;

/// A `SignatureScheme` this client recognizes in a `CertificateVerify`
/// (RFC 8446 section 4.2.3's `rsa_pkcs1_*` values are legal only in
/// `signature_algorithms_cert`, for describing a certificate's own
/// signature -- RFC 8446 section 4.4.3 forbids using them in a live
/// `CertificateVerify`, so [`SignatureScheme::from_code`] recognizes them
/// (for the extension list) but [`verify`] never accepts them here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignatureScheme {
    RsaPkcs1Sha256,
    RsaPkcs1Sha384,
    RsaPkcs1Sha512,
    EcdsaSecp256r1Sha256,
    EcdsaSecp384r1Sha384,
    RsaPssRsaeSha256,
    RsaPssRsaeSha384,
    RsaPssRsaeSha512,
}

impl SignatureScheme {
    pub(crate) fn code(self) -> u16 {
        match self {
            SignatureScheme::RsaPkcs1Sha256 => 0x0401,
            SignatureScheme::RsaPkcs1Sha384 => 0x0501,
            SignatureScheme::RsaPkcs1Sha512 => 0x0601,
            SignatureScheme::EcdsaSecp256r1Sha256 => 0x0403,
            SignatureScheme::EcdsaSecp384r1Sha384 => 0x0503,
            SignatureScheme::RsaPssRsaeSha256 => 0x0804,
            SignatureScheme::RsaPssRsaeSha384 => 0x0805,
            SignatureScheme::RsaPssRsaeSha512 => 0x0806,
        }
    }

    pub(crate) fn from_code(code: u16) -> Option<SignatureScheme> {
        Some(match code {
            0x0401 => SignatureScheme::RsaPkcs1Sha256,
            0x0501 => SignatureScheme::RsaPkcs1Sha384,
            0x0601 => SignatureScheme::RsaPkcs1Sha512,
            0x0403 => SignatureScheme::EcdsaSecp256r1Sha256,
            0x0503 => SignatureScheme::EcdsaSecp384r1Sha384,
            0x0804 => SignatureScheme::RsaPssRsaeSha256,
            0x0805 => SignatureScheme::RsaPssRsaeSha384,
            0x0806 => SignatureScheme::RsaPssRsaeSha512,
            _ => return None,
        })
    }

    fn digest(self, data: &[u8]) -> Digest {
        match self {
            SignatureScheme::RsaPkcs1Sha256 | SignatureScheme::EcdsaSecp256r1Sha256 | SignatureScheme::RsaPssRsaeSha256 => {
                Digest::from_slice(&Sha256::digest(data))
            }
            SignatureScheme::RsaPkcs1Sha384 | SignatureScheme::EcdsaSecp384r1Sha384 | SignatureScheme::RsaPssRsaeSha384 => {
                Digest::from_slice(&Sha384::digest(data))
            }
            SignatureScheme::RsaPkcs1Sha512 | SignatureScheme::RsaPssRsaeSha512 => Digest::from_slice(&Sha512::digest(data)),
        }
    }
}

/// `signature_algorithms` (RFC 8446 section 4.2.3): the schemes this client
/// accepts in a live `CertificateVerify`. No `rsa_pkcs1_*`: RFC 8446 section
/// 4.4.3 reserves those codepoints, when they appear here, for backward
/// compatibility only, never for an actual TLS 1.3 signature.
pub(crate) const SIGNATURE_ALGORITHMS: &[SignatureScheme] = &[
    SignatureScheme::EcdsaSecp256r1Sha256,
    SignatureScheme::EcdsaSecp384r1Sha384,
    SignatureScheme::RsaPssRsaeSha256,
    SignatureScheme::RsaPssRsaeSha384,
    SignatureScheme::RsaPssRsaeSha512,
];

/// `signature_algorithms_cert` (RFC 8446 section 4.2.3): the schemes this
/// client accepts for the signatures *inside* the certificate chain --
/// [`SIGNATURE_ALGORITHMS`] plus the `rsa_pkcs1_*` schemes real CAs still
/// commonly use to sign certificates.
pub(crate) const SIGNATURE_ALGORITHMS_CERT: &[SignatureScheme] = &[
    SignatureScheme::EcdsaSecp256r1Sha256,
    SignatureScheme::EcdsaSecp384r1Sha384,
    SignatureScheme::RsaPssRsaeSha256,
    SignatureScheme::RsaPssRsaeSha384,
    SignatureScheme::RsaPssRsaeSha512,
    SignatureScheme::RsaPkcs1Sha256,
    SignatureScheme::RsaPkcs1Sha384,
    SignatureScheme::RsaPkcs1Sha512,
];

pub(crate) fn encode_list(out: &mut Vec<u8>, schemes: &[SignatureScheme]) {
    out.extend_from_slice(&((schemes.len() * 2) as u16).to_be_bytes());
    for s in schemes {
        out.extend_from_slice(&s.code().to_be_bytes());
    }
}

/// Verifies a `CertificateVerify` signature (RFC 8446 section 4.4.3):
/// `scheme` must be one of [`SIGNATURE_ALGORITHMS`] (never an
/// `rsa_pkcs1_*` code) and must match `leaf_key`'s actual key type -- a
/// mismatch (e.g. an `ecdsa_secp256r1_sha256` signature under an RSA leaf
/// key) is rejected exactly like an invalid signature, not guessed at.
pub(crate) fn verify(leaf_key: &otter_x509::PublicKey, scheme: SignatureScheme, content: &[u8], signature: &[u8]) -> Result<(), TlsError> {
    if !SIGNATURE_ALGORITHMS.contains(&scheme) {
        return Err(TlsError::BadSignature);
    }
    let digest = scheme.digest(content);
    let ok = match (leaf_key, scheme) {
        (otter_x509::PublicKey::Rsa(key), SignatureScheme::RsaPssRsaeSha256) => key.verify_pss(HashAlg::Sha256, &digest, signature).is_ok(),
        (otter_x509::PublicKey::Rsa(key), SignatureScheme::RsaPssRsaeSha384) => key.verify_pss(HashAlg::Sha384, &digest, signature).is_ok(),
        (otter_x509::PublicKey::Rsa(key), SignatureScheme::RsaPssRsaeSha512) => key.verify_pss(HashAlg::Sha512, &digest, signature).is_ok(),
        (otter_x509::PublicKey::P256(key), SignatureScheme::EcdsaSecp256r1Sha256) => key.verify(digest.as_bytes(), signature).is_ok(),
        (otter_x509::PublicKey::P384(key), SignatureScheme::EcdsaSecp384r1Sha384) => key.verify(digest.as_bytes(), signature).is_ok(),
        _ => false,
    };
    if ok { Ok(()) } else { Err(TlsError::BadSignature) }
}
