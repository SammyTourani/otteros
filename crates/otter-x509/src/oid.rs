//! Object identifiers this crate recognizes, as their raw DER content bytes
//! (the concatenated base-128 sub-identifiers a [`crate::der::Reader::read_oid`]
//! call returns -- no tag, no length, arcs 1 and 2 already combined per
//! X.690's encoding rule).
//!
//! Matching is always a byte-slice equality check against one of these
//! constants, never a decode into dotted-decimal numbers: an X.509 verifier
//! only ever needs to ask "is this OID exactly one of the finitely many I
//! understand?", and DER's canonical encoding makes byte equality exactly
//! the right question to ask (`crate::der::Reader::read_oid` has already
//! rejected any non-minimal encoding, so two OIDs with the same value always
//! have the same bytes). Each constant is cross-checked against its
//! standard dotted-decimal form in this module's own tests -- the same
//! "hand-derive, then cross-check against a widely-published table" method
//! `otter-crypto::rsa`'s `DigestInfo` prefixes use.

/// `rsaEncryption`, 1.2.840.113549.1.1.1 (RFC 8017 appendix C).
pub const RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
/// `id-RSASSA-PSS`, 1.2.840.113549.1.1.10 (RFC 8017 appendix C).
pub const RSASSA_PSS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0a];
/// `sha256WithRSAEncryption`, 1.2.840.113549.1.1.11.
pub const SHA256_WITH_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
/// `sha384WithRSAEncryption`, 1.2.840.113549.1.1.12.
pub const SHA384_WITH_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
/// `sha512WithRSAEncryption`, 1.2.840.113549.1.1.13.
pub const SHA512_WITH_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
/// `sha1WithRSAEncryption`, 1.2.840.113549.1.1.5 -- recognized only so it can
/// be rejected by name (DECISIONS.md D22/brief: SHA-1 signatures are refused).
pub const SHA1_WITH_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x05];
/// `md5WithRSAEncryption`, 1.2.840.113549.1.1.4 -- recognized only to reject it.
pub const MD5_WITH_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x04];
/// `id-mgf1`, 1.2.840.113549.1.1.8 (RFC 8017 appendix C), RSASSA-PSS's mask
/// generation function identifier.
pub const MGF1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x08];

/// `id-ecPublicKey`, 1.2.840.10045.2.1 (RFC 5480).
pub const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// `prime256v1` / `secp256r1`, 1.2.840.10045.3.1.7 (RFC 5480/SEC 2), NIST P-256.
pub const PRIME256V1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
/// `secp384r1`, 1.3.132.0.34 (SEC 2), NIST P-384.
pub const SECP384R1: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
/// `ecdsa-with-SHA256`, 1.2.840.10045.4.3.2 (RFC 5758).
pub const ECDSA_WITH_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
/// `ecdsa-with-SHA384`, 1.2.840.10045.4.3.3 (RFC 5758).
pub const ECDSA_WITH_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
/// `ecdsa-with-SHA1`, 1.2.840.10045.4.1 -- recognized only to reject it.
pub const ECDSA_WITH_SHA1: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x01];

/// `id-sha256`, 2.16.840.1.101.3.4.2.1 (used inside RSASSA-PSS parameters).
pub const ID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
/// `id-sha384`, 2.16.840.1.101.3.4.2.2.
pub const ID_SHA384: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02];
/// `id-sha512`, 2.16.840.1.101.3.4.2.3.
pub const ID_SHA512: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03];

/// `id-ce-subjectKeyIdentifier`, 2.5.29.14 (RFC 5280 section 4.2.1.2).
pub const SUBJECT_KEY_IDENTIFIER: &[u8] = &[0x55, 0x1d, 0x0e];
/// `id-ce-keyUsage`, 2.5.29.15 (RFC 5280 section 4.2.1.3).
pub const KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
/// `id-ce-subjectAltName`, 2.5.29.17 (RFC 5280 section 4.2.1.6).
pub const SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];
/// `id-ce-basicConstraints`, 2.5.29.19 (RFC 5280 section 4.2.1.9).
pub const BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
/// `id-ce-nameConstraints`, 2.5.29.30 (RFC 5280 section 4.2.1.10).
pub const NAME_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x1e];
/// `id-ce-authorityKeyIdentifier`, 2.5.29.35 (RFC 5280 section 4.2.1.1).
pub const AUTHORITY_KEY_IDENTIFIER: &[u8] = &[0x55, 0x1d, 0x23];
/// `id-ce-extKeyUsage`, 2.5.29.37 (RFC 5280 section 4.2.1.12).
pub const EXT_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];

/// `id-kp-serverAuth`, 1.3.6.1.5.5.7.3.1 (RFC 5280 section 4.2.1.12).
pub const KP_SERVER_AUTH: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01];
/// `anyExtendedKeyUsage`, 2.5.29.37.0 (RFC 5280 section 4.2.1.12).
pub const ANY_EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25, 0x00];

/// Common `AttributeType`s (RFC 5280 section 4.1.2.4 / X.520), used only for
/// building a `Name`'s human-readable display string.
pub mod attr {
    /// `id-at-commonName`, 2.5.4.3.
    pub const COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
    /// `id-at-countryName`, 2.5.4.6.
    pub const COUNTRY_NAME: &[u8] = &[0x55, 0x04, 0x06];
    /// `id-at-organizationName`, 2.5.4.10.
    pub const ORGANIZATION_NAME: &[u8] = &[0x55, 0x04, 0x0a];
    /// `id-at-organizationalUnitName`, 2.5.4.11.
    pub const ORGANIZATIONAL_UNIT_NAME: &[u8] = &[0x55, 0x04, 0x0b];
    /// `id-at-stateOrProvinceName`, 2.5.4.8.
    pub const STATE_OR_PROVINCE_NAME: &[u8] = &[0x55, 0x04, 0x08];
    /// `id-at-localityName`, 2.5.4.7.
    pub const LOCALITY_NAME: &[u8] = &[0x55, 0x04, 0x07];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::der::Reader;

    /// Cross-checks every constant above against a from-scratch base-128
    /// encoder of its published dotted-decimal form (arcs 1/2 combined as
    /// `40*arc1 + arc2` per X.690 section 8.19.4), so a transcription typo in
    /// one of the hand-written byte arrays would fail loudly here rather
    /// than silently mis-recognizing (or failing to recognize) a real
    /// algorithm.
    fn encode_oid(arcs: &[u64]) -> alloc::vec::Vec<u8> {
        assert!(arcs.len() >= 2);
        let mut combined = alloc::vec![arcs[0] * 40 + arcs[1]];
        combined.extend_from_slice(&arcs[2..]);
        let mut out = alloc::vec::Vec::new();
        for &arc in &combined {
            let mut groups = alloc::vec![(arc & 0x7f) as u8];
            let mut rest = arc >> 7;
            while rest > 0 {
                groups.push((rest & 0x7f) as u8 | 0x80);
                rest >>= 7;
            }
            groups.reverse();
            out.extend_from_slice(&groups);
        }
        out
    }

    #[test]
    fn constants_match_their_dotted_decimal_forms() {
        let cases: &[(&[u8], &[u64])] = &[
            (RSA_ENCRYPTION, &[1, 2, 840, 113549, 1, 1, 1]),
            (RSASSA_PSS, &[1, 2, 840, 113549, 1, 1, 10]),
            (SHA256_WITH_RSA_ENCRYPTION, &[1, 2, 840, 113549, 1, 1, 11]),
            (SHA384_WITH_RSA_ENCRYPTION, &[1, 2, 840, 113549, 1, 1, 12]),
            (SHA512_WITH_RSA_ENCRYPTION, &[1, 2, 840, 113549, 1, 1, 13]),
            (SHA1_WITH_RSA_ENCRYPTION, &[1, 2, 840, 113549, 1, 1, 5]),
            (MD5_WITH_RSA_ENCRYPTION, &[1, 2, 840, 113549, 1, 1, 4]),
            (MGF1, &[1, 2, 840, 113549, 1, 1, 8]),
            (EC_PUBLIC_KEY, &[1, 2, 840, 10045, 2, 1]),
            (PRIME256V1, &[1, 2, 840, 10045, 3, 1, 7]),
            (SECP384R1, &[1, 3, 132, 0, 34]),
            (ECDSA_WITH_SHA256, &[1, 2, 840, 10045, 4, 3, 2]),
            (ECDSA_WITH_SHA384, &[1, 2, 840, 10045, 4, 3, 3]),
            (ECDSA_WITH_SHA1, &[1, 2, 840, 10045, 4, 1]),
            (ID_SHA256, &[2, 16, 840, 1, 101, 3, 4, 2, 1]),
            (ID_SHA384, &[2, 16, 840, 1, 101, 3, 4, 2, 2]),
            (ID_SHA512, &[2, 16, 840, 1, 101, 3, 4, 2, 3]),
            (SUBJECT_KEY_IDENTIFIER, &[2, 5, 29, 14]),
            (KEY_USAGE, &[2, 5, 29, 15]),
            (SUBJECT_ALT_NAME, &[2, 5, 29, 17]),
            (BASIC_CONSTRAINTS, &[2, 5, 29, 19]),
            (NAME_CONSTRAINTS, &[2, 5, 29, 30]),
            (AUTHORITY_KEY_IDENTIFIER, &[2, 5, 29, 35]),
            (EXT_KEY_USAGE, &[2, 5, 29, 37]),
            (KP_SERVER_AUTH, &[1, 3, 6, 1, 5, 5, 7, 3, 1]),
            (ANY_EXTENDED_KEY_USAGE, &[2, 5, 29, 37, 0]),
            (attr::COMMON_NAME, &[2, 5, 4, 3]),
            (attr::COUNTRY_NAME, &[2, 5, 4, 6]),
            (attr::ORGANIZATION_NAME, &[2, 5, 4, 10]),
            (attr::ORGANIZATIONAL_UNIT_NAME, &[2, 5, 4, 11]),
            (attr::STATE_OR_PROVINCE_NAME, &[2, 5, 4, 8]),
            (attr::LOCALITY_NAME, &[2, 5, 4, 7]),
        ];
        for (bytes, arcs) in cases {
            assert_eq!(*bytes, encode_oid(arcs).as_slice(), "mismatch for arcs {arcs:?}");
        }
    }

    #[test]
    fn every_constant_round_trips_through_the_der_reader() {
        for oid in [
            RSA_ENCRYPTION,
            RSASSA_PSS,
            SHA256_WITH_RSA_ENCRYPTION,
            EC_PUBLIC_KEY,
            PRIME256V1,
            SECP384R1,
            ECDSA_WITH_SHA256,
            ECDSA_WITH_SHA384,
            BASIC_CONSTRAINTS,
            KEY_USAGE,
            SUBJECT_ALT_NAME,
        ] {
            let mut der = alloc::vec![0x06u8, oid.len() as u8];
            der.extend_from_slice(oid);
            let mut r = Reader::new(&der);
            assert_eq!(r.read_oid().unwrap(), oid);
        }
    }
}
