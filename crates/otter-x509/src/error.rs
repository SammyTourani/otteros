//! The one error type every fallible operation in this crate returns.
//!
//! A TLS client that finds anything wrong with a certificate chain does the
//! same thing regardless of which thing was wrong: abort the handshake
//! (DECISIONS.md D22). Keeping one flat, `Copy` enum (rather than one error
//! type per module) makes that "any failure aborts" posture the obviously
//! correct way to use this crate's API, exactly as `otter-crypto`'s
//! `RsaError`/`EcdsaError` do for signature verification alone. Variants are
//! still specific enough that tests can assert *which* rule rejected a
//! fixture, and that the x509-limbo harness can map an unsupported feature
//! to a specific cause.

use crate::der::DerError;

/// Why a DER value, certificate, or certificate chain was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X509Error {
    /// A DER structural rule was violated (see [`DerError`]).
    Der(DerError),
    /// `version` was not `2` (v3) -- the only version this crate's
    /// extension-dependent validation logic supports.
    UnsupportedVersion,
    /// The certificate's inner (`TBSCertificate.signature`) and outer
    /// (`Certificate.signatureAlgorithm`) `AlgorithmIdentifier`s did not
    /// match byte-for-byte (RFC 5280 section 4.1.1.2).
    SignatureAlgorithmMismatch,
    /// A signature algorithm this crate does not implement, or does not
    /// trust (SHA-1, MD5), or whose parameters (e.g. a non-matching
    /// RSASSA-PSS salt length, a non-default trailer field) this crate does
    /// not support.
    UnsupportedSignatureAlgorithm,
    /// `SubjectPublicKeyInfo` named an algorithm or curve this crate does
    /// not implement (only rsaEncryption and id-ecPublicKey with
    /// prime256v1/secp384r1 are supported).
    UnsupportedPublicKey,
    /// A public key's encoding was structurally invalid, or `otter-crypto`
    /// rejected it (bad modulus/exponent, off-curve or infinity point).
    InvalidPublicKey,
    /// A `critical` extension this crate does not recognize was present
    /// (RFC 5280 section 4.2: a certificate-using system MUST reject the
    /// certificate if it cannot process a critical extension).
    UnknownCriticalExtension,
    /// An extension's `extnValue` did not parse as the DER structure its
    /// OID implies, or a well-known extension appeared more than once.
    MalformedExtension,
    /// A `Name` (issuer or subject) did not parse as a well-formed
    /// `RDNSequence`.
    InvalidName,
    /// No certification path from the leaf to any trust anchor could be
    /// built (every candidate issuer either did not exist, did not verify,
    /// or failed one of RFC 5280's chain-building constraints).
    NoPathFound,
    /// A signature in the chain, or the certificate signature itself, did
    /// not verify under the claimed issuer's public key.
    SignatureVerificationFailed,
    /// A certificate's validity period did not cover the given time.
    CertificateExpired,
    /// A certificate's validity period had not yet started at the given time.
    CertificateNotYetValid,
    /// A certificate was used as an issuer (to sign another certificate) but
    /// its `basicConstraints` did not assert `cA: TRUE`.
    NotACertificateAuthority,
    /// A `pathLenConstraint` was violated: an issuer does not permit as many
    /// subordinate CA certificates as appear below it in the path.
    PathLengthExceeded,
    /// An issuer's `keyUsage` extension was present but did not assert
    /// `keyCertSign`.
    MissingKeyCertSign,
    /// The leaf certificate's `basicConstraints` asserted `cA: TRUE` (a CA
    /// certificate cannot be used as a TLS server's end-entity certificate).
    LeafIsCertificateAuthority,
    /// The leaf certificate's `extKeyUsage` was present but did not include
    /// `id-kp-serverAuth` or `anyExtendedKeyUsage`.
    ExtendedKeyUsageMismatch,
    /// A `nameConstraints` extension used a form other than a `dNSName`
    /// subtree with the default minimum and no maximum -- any such form
    /// makes every path through that certificate invalid, because this
    /// crate cannot correctly enforce it.
    UnsupportedNameConstraint,
    /// A subordinate certificate's `dNSName` `subjectAltName` was outside a
    /// `nameConstraints` permitted subtree, or inside an excluded one.
    NameConstraintViolation,
    /// The chain building search exceeded its maximum depth without
    /// reaching a trust anchor.
    PathDepthExceeded,
    /// The certificate chain was empty.
    EmptyChain,
    /// The certificate chain had more certificates than this crate's fixed
    /// limit (protects against unbounded work on hostile input).
    ChainTooLong,
    /// The leaf certificate had no `subjectAltName` extension (RFC 6125:
    /// this crate never falls back to the deprecated `commonName` match), or
    /// its `subjectAltName` did not contain an entry matching the requested
    /// host.
    HostnameMismatch,
    /// The host name given to [`crate::verify_server_chain`] was not a
    /// syntactically valid DNS name or IP-address literal.
    InvalidHostName,
}

impl From<DerError> for X509Error {
    fn from(e: DerError) -> X509Error {
        X509Error::Der(e)
    }
}
