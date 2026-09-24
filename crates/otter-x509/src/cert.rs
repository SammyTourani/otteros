//! `Certificate` (RFC 5280 section 4.1): parses one whole DER-encoded X.509
//! certificate into every field [`crate::path`]'s chain validation needs,
//! borrowing from the input buffer throughout (no copying) except where
//! [`crate::name::Name`]/[`crate::extensions`] already do the same. Every
//! field is `pub(crate)`: this type is an implementation detail of this
//! crate's own path-building/validation logic, never handed to a caller of
//! [`crate::verify_server_chain`] (which returns the much smaller, owned
//! [`crate::path::VerifiedChain`] instead).
//!
//! Structural strictness enforced here, beyond what [`crate::der`] already
//! guarantees: `version` must be `2` (v3) -- this crate's extension-driven
//! validation logic has nothing sensible to do with a v1/v2 certificate;
//! the inner (`TBSCertificate.signature`) and outer
//! (`Certificate.signatureAlgorithm`) `AlgorithmIdentifier`s must be
//! byte-identical (RFC 5280 section 4.1.1.2); no extension OID may repeat;
//! any *critical* extension this crate does not implement is rejected
//! outright (RFC 5280 section 4.2).

use alloc::vec::Vec;

use crate::der::{DerError, Reader, tag};
use crate::error::X509Error;
use crate::extensions::{
    self, BasicConstraints, ExtKeyUsage, KeyUsage, NameConstraints, SubjectAltName, parse_extensions,
};
use crate::name::Name;
use crate::oid;
use crate::sigalg::{self, SignatureAlgorithm};
use crate::spki::{self, PublicKey};
use crate::time;

fn require_empty(r: &Reader<'_>) -> Result<(), DerError> {
    if r.is_empty() { Ok(()) } else { Err(DerError::TrailingData) }
}

/// A parsed X.509v3 certificate, borrowing from the DER bytes it was parsed
/// from.
pub(crate) struct Certificate<'a> {
    /// The raw `TBSCertificate` DER (tag+length stripped): what a signature
    /// over this certificate covers.
    pub(crate) tbs_raw: &'a [u8],
    /// `serialNumber`'s raw non-negative big-endian bytes. Parsed and
    /// validated (RFC 5280 section 4.1.2.2) as part of this crate's strict
    /// DER posture, but not otherwise consulted by path validation -- no
    /// rule in the brief's scope keys off it (a serial only matters for
    /// revocation checking, out of scope here).
    #[allow(dead_code)]
    pub(crate) serial: &'a [u8],
    /// The (identical) inner/outer signature algorithm.
    pub(crate) sig_alg: SignatureAlgorithm,
    pub(crate) issuer: Name<'a>,
    pub(crate) not_before: i64,
    pub(crate) not_after: i64,
    pub(crate) subject: Name<'a>,
    pub(crate) public_key: PublicKey,
    pub(crate) basic_constraints: Option<BasicConstraints>,
    pub(crate) key_usage: Option<KeyUsage<'a>>,
    pub(crate) ext_key_usage: Option<ExtKeyUsage>,
    pub(crate) san: Option<SubjectAltName<'a>>,
    pub(crate) akid: Option<&'a [u8]>,
    pub(crate) skid: Option<&'a [u8]>,
    pub(crate) name_constraints: Option<NameConstraints<'a>>,
    /// `signatureValue`'s raw bytes (the BIT STRING's content; this crate
    /// requires zero unused bits, true of every signature scheme it
    /// implements).
    pub(crate) signature: &'a [u8],
}

impl<'a> Certificate<'a> {
    /// Whether `basicConstraints` asserts `cA: TRUE` (absent -> `false`).
    pub(crate) fn is_ca(&self) -> bool {
        self.basic_constraints.is_some_and(|bc| bc.is_ca)
    }

    /// Parses one DER-encoded `Certificate`. `der` must contain exactly one
    /// certificate and nothing else.
    pub(crate) fn parse(der: &'a [u8]) -> Result<Certificate<'a>, X509Error> {
        let mut top = Reader::new(der);
        let mut cert = top.read_sequence()?;
        require_empty(&top)?;

        // `tbs_raw` must be the *entire* TBSCertificate encoding (tag and
        // length included): that is exactly what `signatureValue` is a
        // signature over (RFC 5280 section 4.1.1.3), not just its content.
        let tbs_raw = cert.read_tlv_bytes()?;
        if tbs_raw.first() != Some(&tag::SEQUENCE) {
            return Err(DerError::UnexpectedTag.into());
        }

        let outer_alg_tlv = cert.read_tlv()?;
        if outer_alg_tlv.tag != tag::SEQUENCE {
            return Err(DerError::UnexpectedTag.into());
        }

        let signature = cert.read_bit_string_bytes()?;
        require_empty(&cert)?;

        let mut tbs = Reader::new(tbs_raw).read_sequence()?;

        let version = match tbs.read_explicit(0)? {
            Some(mut v) => {
                let value = v.read_small_uint()?;
                require_empty(&v)?;
                value
            }
            None => 0,
        };
        if version != 2 {
            return Err(X509Error::UnsupportedVersion);
        }

        let serial = tbs.read_uint_bytes()?;

        let inner_alg_tlv = tbs.read_tlv()?;
        if inner_alg_tlv.tag != tag::SEQUENCE {
            return Err(DerError::UnexpectedTag.into());
        }
        if inner_alg_tlv.content != outer_alg_tlv.content {
            return Err(X509Error::SignatureAlgorithmMismatch);
        }
        let sig_alg = sigalg::parse_algorithm_identifier(inner_alg_tlv.content)?;

        let issuer = Name::parse(&mut tbs)?;

        let mut validity = tbs.read_sequence()?;
        let not_before = time::read_time(&mut validity)?;
        let not_after = time::read_time(&mut validity)?;
        require_empty(&validity)?;
        if not_after < not_before {
            return Err(X509Error::CertificateExpired);
        }

        let subject = Name::parse(&mut tbs)?;
        let public_key = spki::parse_spki(&mut tbs)?;

        // issuerUniqueID [1] / subjectUniqueID [2]: legal since v2, unused by
        // this crate's validation logic, just skipped if present.
        tbs.read_optional_tag(tag::context_primitive(1))?;
        tbs.read_optional_tag(tag::context_primitive(2))?;

        let mut basic_constraints = None;
        let mut key_usage = None;
        let mut ext_key_usage = None;
        let mut san = None;
        let mut akid = None;
        let mut skid = None;
        let mut name_constraints = None;

        if let Some(mut ext_wrapper) = tbs.read_explicit(3)? {
            let list = parse_extensions(&mut ext_wrapper)?;
            require_empty(&ext_wrapper)?;

            let mut seen: Vec<&[u8]> = Vec::with_capacity(list.len());
            for ext in &list {
                if seen.contains(&ext.oid) {
                    return Err(X509Error::MalformedExtension);
                }
                seen.push(ext.oid);

                if ext.oid == oid::BASIC_CONSTRAINTS {
                    let bc = extensions::parse_basic_constraints(ext.value)?;
                    // RFC 5280 section 4.2.1.9: "conforming CAs MUST mark
                    // the extension as critical" whenever cA is asserted.
                    if bc.is_ca && !ext.critical {
                        return Err(X509Error::MalformedExtension);
                    }
                    basic_constraints = Some(bc);
                } else if ext.oid == oid::KEY_USAGE {
                    key_usage = Some(extensions::parse_key_usage(ext.value)?);
                } else if ext.oid == oid::EXT_KEY_USAGE {
                    ext_key_usage = Some(extensions::parse_ext_key_usage(ext.value)?);
                } else if ext.oid == oid::SUBJECT_ALT_NAME {
                    san = Some(extensions::parse_subject_alt_name(ext.value)?);
                } else if ext.oid == oid::AUTHORITY_KEY_IDENTIFIER {
                    akid = extensions::parse_authority_key_identifier(ext.value)?;
                } else if ext.oid == oid::SUBJECT_KEY_IDENTIFIER {
                    skid = Some(extensions::parse_subject_key_identifier(ext.value)?);
                } else if ext.oid == oid::NAME_CONSTRAINTS {
                    // RFC 5280 section 4.2.1.10: "conforming CAs MUST mark
                    // this extension as critical".
                    if !ext.critical {
                        return Err(X509Error::MalformedExtension);
                    }
                    name_constraints = Some(extensions::parse_name_constraints(ext.value)?);
                } else if ext.critical {
                    return Err(X509Error::UnknownCriticalExtension);
                }
            }
        }

        require_empty(&tbs)?;

        let is_ca = basic_constraints.is_some_and(|bc| bc.is_ca);
        // RFC 5280 section 4.2.1.9: "If the cA boolean is not asserted,
        // then the keyCertSign bit in the key usage extension MUST NOT be
        // asserted."
        if !is_ca && key_usage.is_some_and(|ku| ku.key_cert_sign()) {
            return Err(X509Error::MalformedExtension);
        }
        // RFC 5280 section 4.1.2.6: a CA's subject "MUST be populated with
        // a non-empty distinguished name".
        if is_ca && subject.raw().is_empty() {
            return Err(X509Error::InvalidName);
        }

        Ok(Certificate {
            tbs_raw,
            serial,
            sig_alg,
            issuer,
            not_before,
            not_after,
            subject,
            public_key,
            basic_constraints,
            key_usage,
            ext_key_usage,
            san,
            akid,
            skid,
            name_constraints,
            signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_trailing_bytes_after_the_certificate() {
        // Two consecutive INTEGER TLVs are not a valid Certificate at all,
        // but this specifically exercises the "trailing data after a
        // complete top-level value" rule using a cheap non-certificate shape.
        let der = [0x30, 0x03, 0x02, 0x01, 0x00, 0xff];
        assert!(matches!(Certificate::parse(&der), Err(X509Error::Der(DerError::TrailingData))));
    }

    #[test]
    fn empty_input_is_rejected_not_panicked() {
        assert!(Certificate::parse(&[]).is_err());
    }

    // `is_ca()`, version enforcement, algorithm-mismatch rejection, duplicate-
    // and unknown-critical-extension rejection are all exercised end-to-end
    // against real certificates in tests/test_pki.rs and tests/robustness.rs
    // -- hand-building a whole valid Certificate's worth of nested SEQUENCEs
    // byte-by-byte here would just re-implement (and risk diverging from) the
    // fixtures scripts/make-test-pki.sh already generates.
}
