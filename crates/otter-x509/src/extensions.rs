//! X.509v3 extensions (RFC 5280 section 4.2): the generic `Extension` list,
//! plus dedicated parsers for the seven extensions this crate understands --
//! `basicConstraints`, `keyUsage`, `extKeyUsage`, `subjectAltName`,
//! `authorityKeyIdentifier`, `subjectKeyIdentifier`, `nameConstraints`.
//! [`crate::cert`] walks the generic list, dispatches known OIDs to these
//! parsers, and rejects any *other* OID marked `critical` (RFC 5280 section
//! 4.2: "a certificate-using system MUST reject the certificate if it
//! encounters a critical extension it does not recognize").
//!
//! `nameConstraints` is the one extension this crate only partially
//! implements (dNSName subtrees with the default minimum and no maximum,
//! per the brief): rather than fail to parse a certificate whose
//! `nameConstraints` uses a form this crate cannot enforce,
//! [`parse_name_constraints`] sets [`NameConstraints::unsupported`] and lets
//! [`crate::path`]'s chain-building search treat that certificate as
//! unusable as an issuer (the same way a bad signature would be) --
//! backtracking to another candidate if one exists, and only failing the
//! whole chain if every path runs through a certificate whose constraints it
//! cannot honour.

use alloc::vec::Vec;

use crate::der::{DerError, Reader, decode_uint, tag};
use crate::error::X509Error;
use crate::oid;

fn require_empty(r: &Reader<'_>) -> Result<(), DerError> {
    if r.is_empty() { Ok(()) } else { Err(DerError::TrailingData) }
}

/// One `Extension` (RFC 5280 section 4.1.2.9), not yet interpreted.
#[derive(Clone, Copy, Debug)]
pub struct Extension<'a> {
    /// `extnID`'s raw OID content bytes (see [`crate::oid`]).
    pub oid: &'a [u8],
    /// `critical`, defaulting to `false` when absent.
    pub critical: bool,
    /// `extnValue`'s content: itself DER, whose shape depends on `oid`.
    pub value: &'a [u8],
}

/// Reads an `Extensions` (`SEQUENCE SIZE (1..MAX) OF Extension`), i.e. `r`
/// must be positioned right before `Extensions`' own `SEQUENCE` tag (as
/// [`crate::cert::Certificate::parse`] leaves it after unwrapping
/// `TBSCertificate`'s `[3] EXPLICIT` wrapper: `EXPLICIT` adds exactly one
/// more layer of tagging around the `Extensions` value itself).
pub fn parse_extensions<'a>(r: &mut Reader<'a>) -> Result<Vec<Extension<'a>>, X509Error> {
    let mut r = r.read_sequence()?;
    let mut list = Vec::new();
    while !r.is_empty() {
        let mut ext = r.read_sequence()?;
        let oid = ext.read_oid()?;
        let critical = ext.read_boolean_default_false()?;
        let value = ext.read_octet_string()?;
        require_empty(&ext)?;
        list.push(Extension { oid, critical, value });
    }
    if list.is_empty() {
        return Err(X509Error::MalformedExtension);
    }
    Ok(list)
}

/// `BasicConstraints` (RFC 5280 section 4.2.1.9).
#[derive(Clone, Copy, Debug)]
pub struct BasicConstraints {
    /// `cA`, defaulting to `false` when absent.
    pub is_ca: bool,
    /// `pathLenConstraint`, if present. Meaningful only when `is_ca` is true.
    pub path_len: Option<u64>,
}

/// Parses a `basicConstraints` extension value.
pub fn parse_basic_constraints(value: &[u8]) -> Result<BasicConstraints, X509Error> {
    let mut r = Reader::new(value);
    let mut seq = r.read_sequence()?;
    let is_ca = seq.read_boolean_default_false()?;
    let path_len = match seq.peek_tag() {
        Some(t) if t == tag::INTEGER => Some(seq.read_small_uint()?),
        _ => None,
    };
    require_empty(&seq)?;
    require_empty(&r)?;
    Ok(BasicConstraints { is_ca, path_len })
}

/// `KeyUsage` (RFC 5280 section 4.2.1.3): a `NamedBitList` `BIT STRING`
/// borrowed straight from the certificate's DER, decoded bit-by-bit on
/// demand rather than eagerly into a fixed-width flags type, so it copes
/// with encodings of any length (trailing zero bits/bytes are conventionally
/// trimmed, but this crate does not require that: a missing byte and a
/// present-but-zero byte mean the same thing here).
#[derive(Clone, Copy, Debug)]
pub struct KeyUsage<'a> {
    bytes: &'a [u8],
}

impl<'a> KeyUsage<'a> {
    fn bit(&self, n: usize) -> bool {
        let byte_idx = n / 8;
        let mask = 0x80u8 >> (n % 8);
        self.bytes.get(byte_idx).is_some_and(|b| b & mask != 0)
    }

    /// Bit 0: `digitalSignature`.
    pub fn digital_signature(&self) -> bool {
        self.bit(0)
    }
    /// Bit 5: `keyCertSign` -- required of any certificate used to sign
    /// another certificate, when `keyUsage` is present at all.
    pub fn key_cert_sign(&self) -> bool {
        self.bit(5)
    }
    /// Bit 6: `cRLSign`.
    pub fn crl_sign(&self) -> bool {
        self.bit(6)
    }
}

/// Parses a `keyUsage` extension value.
pub fn parse_key_usage(value: &[u8]) -> Result<KeyUsage<'_>, X509Error> {
    let mut r = Reader::new(value);
    let (_unused, bytes) = r.read_bit_string()?;
    require_empty(&r)?;
    if bytes.is_empty() {
        return Err(X509Error::MalformedExtension); // no bits asserted at all
    }
    Ok(KeyUsage { bytes })
}

/// `ExtKeyUsage` (RFC 5280 section 4.2.1.12), reduced to the one fact
/// [`crate::path`] needs: whether TLS server authentication is among the
/// asserted purposes.
#[derive(Clone, Copy, Debug)]
pub struct ExtKeyUsage {
    /// Whether `id-kp-serverAuth` or `anyExtendedKeyUsage` was listed.
    pub server_auth: bool,
}

/// Parses an `extKeyUsage` extension value.
pub fn parse_ext_key_usage(value: &[u8]) -> Result<ExtKeyUsage, X509Error> {
    let mut r = Reader::new(value);
    let mut seq = r.read_sequence()?;
    let mut server_auth = false;
    let mut count = 0usize;
    while !seq.is_empty() {
        let purpose = seq.read_oid()?;
        if purpose == oid::KP_SERVER_AUTH || purpose == oid::ANY_EXTENDED_KEY_USAGE {
            server_auth = true;
        }
        count += 1;
    }
    require_empty(&r)?;
    if count == 0 {
        return Err(X509Error::MalformedExtension);
    }
    Ok(ExtKeyUsage { server_auth })
}

/// One `GeneralName` this crate acts on (RFC 5280 section 4.2.1.6); every
/// other `GeneralName` choice (`otherName`, `rfc822Name`, `x400Address`,
/// `directoryName`, `ediPartyName`, `uniformResourceIdentifier`,
/// `registeredID`) is skipped while parsing a `subjectAltName` or
/// `nameConstraints` subtree list -- neither RFC 6125 host-name matching nor
/// this crate's `dNSName`-only `nameConstraints` support needs them.
#[derive(Clone, Copy, Debug)]
pub enum GeneralName<'a> {
    /// `[2] dNSName` (`IA5String` content, not yet validated as ASCII --
    /// see [`crate::hostname`]).
    Dns(&'a [u8]),
    /// `[7] iPAddress` (raw address bytes, conventionally 4 or 16 of them --
    /// not yet length-checked; see [`crate::hostname`]).
    Ip(&'a [u8]),
}

/// `subjectAltName` (RFC 5280 section 4.2.1.6), reduced to the `dNSName` and
/// `iPAddress` choices RFC 6125 host-name checking uses.
#[derive(Clone, Debug)]
pub struct SubjectAltName<'a> {
    /// Every `dNSName`/`iPAddress` entry, in encoded order.
    pub names: Vec<GeneralName<'a>>,
}

/// Parses a `subjectAltName` extension value.
pub fn parse_subject_alt_name<'a>(value: &'a [u8]) -> Result<SubjectAltName<'a>, X509Error> {
    let mut r = Reader::new(value);
    let mut seq = r.read_sequence()?;
    let mut names = Vec::new();
    let mut total = 0usize;
    while !seq.is_empty() {
        let tlv = seq.read_tlv()?;
        total += 1;
        if tlv.tag == tag::context_primitive(2) {
            names.push(GeneralName::Dns(tlv.content));
        } else if tlv.tag == tag::context_primitive(7) {
            names.push(GeneralName::Ip(tlv.content));
        }
    }
    require_empty(&r)?;
    if total == 0 {
        return Err(X509Error::MalformedExtension);
    }
    Ok(SubjectAltName { names })
}

/// Parses an `authorityKeyIdentifier` extension value, returning its
/// `keyIdentifier` field (`[0] IMPLICIT OCTET STRING`) if present. The other
/// two fields (`authorityCertIssuer`, `authorityCertSerialNumber`) are
/// skipped: this crate uses the key identifier only as an optional filter
/// narrowing path-building candidates by name (brief M8-T4), never as the
/// sole criterion.
pub fn parse_authority_key_identifier(value: &[u8]) -> Result<Option<&[u8]>, X509Error> {
    let mut r = Reader::new(value);
    let mut seq = r.read_sequence()?;
    let key_id = seq.read_optional_tag(tag::context_primitive(0))?;
    // Skip authorityCertIssuer ([1]) / authorityCertSerialNumber ([2]) if present.
    while !seq.is_empty() {
        seq.read_tlv()?;
    }
    require_empty(&r)?;
    Ok(key_id)
}

/// Parses a `subjectKeyIdentifier` extension value (itself just an
/// `OCTET STRING`, so `extnValue`'s content is an `OCTET STRING`-in-an-
/// `OCTET STRING`).
pub fn parse_subject_key_identifier(value: &[u8]) -> Result<&[u8], X509Error> {
    let mut r = Reader::new(value);
    let key_id = r.read_octet_string()?;
    require_empty(&r)?;
    Ok(key_id)
}

/// `NameConstraints` (RFC 5280 section 4.2.1.10), `dNSName` subtrees only.
#[derive(Clone, Debug)]
pub struct NameConstraints<'a> {
    /// Every permitted `dNSName` subtree's raw (`IA5String`) bytes.
    pub permitted_dns: Vec<&'a [u8]>,
    /// Every excluded `dNSName` subtree's raw (`IA5String`) bytes.
    pub excluded_dns: Vec<&'a [u8]>,
    /// Set if any subtree (permitted or excluded) used a form other than a
    /// bare `dNSName` (a non-`dNSName` base, a non-default `minimum`, or any
    /// `maximum`) -- this crate cannot enforce those, so per the brief, any
    /// path through the certificate that carries this extension is invalid.
    pub unsupported: bool,
}

/// Parses a `nameConstraints` extension value.
pub fn parse_name_constraints<'a>(value: &'a [u8]) -> Result<NameConstraints<'a>, X509Error> {
    let mut r = Reader::new(value);
    let mut seq = r.read_sequence()?;
    let mut permitted_dns = Vec::new();
    let mut excluded_dns = Vec::new();
    let mut unsupported = false;
    let mut any = false;

    if let Some(mut permitted) = seq.read_explicit(0)? {
        any = true;
        parse_general_subtrees(&mut permitted, &mut permitted_dns, &mut unsupported)?;
    }
    if let Some(mut excluded) = seq.read_explicit(1)? {
        any = true;
        parse_general_subtrees(&mut excluded, &mut excluded_dns, &mut unsupported)?;
    }
    require_empty(&seq)?;
    require_empty(&r)?;
    if !any {
        return Err(X509Error::MalformedExtension); // at least one subtree list is required
    }
    Ok(NameConstraints { permitted_dns, excluded_dns, unsupported })
}

fn parse_general_subtrees<'a>(
    r: &mut Reader<'a>,
    dns_out: &mut Vec<&'a [u8]>,
    unsupported: &mut bool,
) -> Result<(), X509Error> {
    let mut count = 0usize;
    while !r.is_empty() {
        count += 1;
        let mut subtree = r.read_sequence()?;
        let base = subtree.read_tlv()?;
        let minimum = subtree.read_optional_tag(tag::context_primitive(0))?;
        let maximum = subtree.read_optional_tag(tag::context_primitive(1))?;
        require_empty(&subtree)?;

        let minimum_is_default = match minimum {
            None => true,
            Some(bytes) => decode_uint(bytes).is_ok_and(|v| v == 0),
        };
        if base.tag == tag::context_primitive(2) && maximum.is_none() && minimum_is_default {
            dns_out.push(base.content);
        } else {
            *unsupported = true;
        }
    }
    if count == 0 {
        return Err(X509Error::MalformedExtension);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(content: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec![0x30u8, content.len() as u8];
        out.extend_from_slice(content);
        out
    }

    #[test]
    fn basic_constraints_ca_with_path_len() {
        // SEQUENCE { BOOLEAN TRUE, INTEGER 0 }
        let value = seq(&[0x01, 0x01, 0xff, 0x02, 0x01, 0x00]);
        let bc = parse_basic_constraints(&value).unwrap();
        assert!(bc.is_ca);
        assert_eq!(bc.path_len, Some(0));
    }

    #[test]
    fn basic_constraints_defaults_to_not_ca() {
        let value = seq(&[]);
        let bc = parse_basic_constraints(&value).unwrap();
        assert!(!bc.is_ca);
        assert_eq!(bc.path_len, None);
    }

    #[test]
    fn key_usage_bit_positions() {
        // digitalSignature(0) and keyCertSign(5) set: 1000 0100 = 0x84.
        let value = seq_bitstring(0, &[0b1000_0100]);
        let ku = parse_key_usage(&value).unwrap();
        assert!(ku.digital_signature());
        assert!(ku.key_cert_sign());
        assert!(!ku.crl_sign());
    }

    fn seq_bitstring(unused: u8, bytes: &[u8]) -> alloc::vec::Vec<u8> {
        let mut out = alloc::vec![0x03u8, (bytes.len() + 1) as u8, unused];
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn ext_key_usage_recognizes_server_auth() {
        let mut oid_tlv = alloc::vec![0x06u8, oid::KP_SERVER_AUTH.len() as u8];
        oid_tlv.extend_from_slice(oid::KP_SERVER_AUTH);
        let value = seq(&oid_tlv);
        let eku = parse_ext_key_usage(&value).unwrap();
        assert!(eku.server_auth);
    }

    #[test]
    fn ext_key_usage_rejects_unrelated_purpose_only() {
        // id-kp-clientAuth, 1.3.6.1.5.5.7.3.2 -- not serverAuth.
        let client_auth = [0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x02];
        let mut oid_tlv = alloc::vec![0x06u8, client_auth.len() as u8];
        oid_tlv.extend_from_slice(&client_auth);
        let value = seq(&oid_tlv);
        let eku = parse_ext_key_usage(&value).unwrap();
        assert!(!eku.server_auth);
    }

    #[test]
    fn subject_alt_name_extracts_dns_and_ip() {
        let mut content = alloc::vec![0x82u8, 3];
        content.extend_from_slice(b"abc");
        content.extend([0x87, 4, 127, 0, 0, 1]);
        let value = seq(&content);
        let san = parse_subject_alt_name(&value).unwrap();
        assert_eq!(san.names.len(), 2);
        assert!(matches!(san.names[0], GeneralName::Dns(b) if b == b"abc"));
        assert!(matches!(san.names[1], GeneralName::Ip(b) if b == [127,0,0,1]));
    }

    #[test]
    fn subject_alt_name_rejects_empty() {
        let value = seq(&[]);
        assert_eq!(parse_subject_alt_name(&value).err(), Some(X509Error::MalformedExtension));
    }

    /// Wraps `content` as `[n] EXPLICIT` (a constructed context tag).
    fn explicit(n: u8, content: &[u8]) -> alloc::vec::Vec<u8> {
        let mut v = alloc::vec![0xa0u8 | n, content.len() as u8];
        v.extend_from_slice(content);
        v
    }

    #[test]
    fn name_constraints_accepts_plain_dns_subtree() {
        // permitted [0] { SEQUENCE { base [2] "example.com" } }
        let mut base = alloc::vec![0x82u8, 11];
        base.extend_from_slice(b"example.com");
        let subtree = seq(&base);
        let value = seq(&explicit(0, &subtree));
        let nc = parse_name_constraints(&value).unwrap();
        assert!(!nc.unsupported);
        assert_eq!(nc.permitted_dns, alloc::vec![b"example.com".as_slice()]);
    }

    #[test]
    fn name_constraints_flags_non_dns_base_as_unsupported() {
        // permitted [0] { SEQUENCE { base [7] iPAddress 4 bytes } }
        let base = alloc::vec![0x87u8, 4, 10, 0, 0, 0];
        let subtree = seq(&base);
        let value = seq(&explicit(0, &subtree));
        let nc = parse_name_constraints(&value).unwrap();
        assert!(nc.unsupported);
        assert!(nc.permitted_dns.is_empty());
    }

    #[test]
    fn name_constraints_flags_maximum_present_as_unsupported() {
        let mut base = alloc::vec![0x82u8, 3];
        base.extend_from_slice(b"abc");
        base.extend([0x81, 0x01, 0x02]); // maximum [1] = 2
        let subtree = seq(&base);
        let value = seq(&explicit(0, &subtree));
        let nc = parse_name_constraints(&value).unwrap();
        assert!(nc.unsupported);
    }

    #[test]
    fn authority_key_identifier_extracts_key_id() {
        let mut content = alloc::vec![0x80u8, 4];
        content.extend([1, 2, 3, 4]);
        let value = seq(&content);
        let akid = parse_authority_key_identifier(&value).unwrap();
        assert_eq!(akid, Some(&[1, 2, 3, 4][..]));
    }

    #[test]
    fn subject_key_identifier_unwraps_octet_string() {
        let mut value = alloc::vec![0x04u8, 4];
        value.extend([9, 8, 7, 6]);
        let skid = parse_subject_key_identifier(&value).unwrap();
        assert_eq!(skid, &[9, 8, 7, 6]);
    }
}
