//! RFC 6125 host-name verification: does a leaf certificate's
//! `subjectAltName` identify the host a TLS client is connecting to?
//!
//! Per the brief (and RFC 6125's own recommendation, "Notes" section 6.4.4):
//! `subjectAltName` only, never a fallback to the deprecated `commonName`
//! match; a certificate with no `subjectAltName` at all never matches
//! anything. Matching is ASCII case-insensitive. Exactly one wildcard form
//! is accepted: the entire left-most label is `*`, matching exactly one
//! (non-empty) label, with at least two labels remaining after it -- so
//! `*.example.com` matches `www.example.com` but `*.com` never matches
//! anything (this crate does not consult a public-suffix list; the brief's
//! simpler and stricter rule is "reject any two-label wildcard outright").
//! An IP-literal host (IPv4 or IPv6, parsed from scratch below -- no
//! `std::net`, this module builds for `no_std`) matches only `iPAddress`
//! `subjectAltName` entries with the identical address bytes, never a
//! `dNSName`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::extensions::{GeneralName, SubjectAltName};

/// A host name parsed as an IP-address literal rather than a DNS name.
enum IpLiteral {
    V4([u8; 4]),
    V6([u8; 16]),
}

impl IpLiteral {
    fn as_bytes(&self) -> &[u8] {
        match self {
            IpLiteral::V4(b) => b,
            IpLiteral::V6(b) => b,
        }
    }
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut n = 0;
    for part in s.split('.') {
        if n == 4 || part.is_empty() || part.len() > 3 {
            return None;
        }
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if part.len() > 1 && part.starts_with('0') {
            return None; // no leading zeros: avoids octal-vs-decimal ambiguity
        }
        let v: u32 = part.parse().ok()?;
        if v > 255 {
            return None;
        }
        out[n] = v as u8;
        n += 1;
    }
    if n == 4 { Some(out) } else { None }
}

fn parse_ipv6_groups(s: &str) -> Option<Vec<u16>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    s.split(':')
        .map(|g| {
            if g.is_empty() || g.len() > 4 || !g.bytes().all(|b| b.is_ascii_hexdigit()) {
                None
            } else {
                u16::from_str_radix(g, 16).ok()
            }
        })
        .collect()
}

fn parse_ipv6(s: &str) -> Option<[u8; 16]> {
    if s.matches("::").count() > 1 {
        return None; // at most one "::" compression
    }
    let groups = if let Some(idx) = s.find("::") {
        let head = parse_ipv6_groups(&s[..idx])?;
        let tail = parse_ipv6_groups(&s[idx + 2..])?;
        if head.len() + tail.len() > 7 {
            return None; // "::" must stand in for at least one group
        }
        let mut full = head;
        full.resize(8 - tail.len(), 0);
        full.extend(tail);
        full
    } else {
        parse_ipv6_groups(s)?
    };
    if groups.len() != 8 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, g) in groups.iter().enumerate() {
        out[2 * i] = (g >> 8) as u8;
        out[2 * i + 1] = (g & 0xff) as u8;
    }
    Some(out)
}

/// Parses `host` as an IPv4 or IPv6 address literal, `None` if it is neither
/// (in which case it is a candidate DNS name instead). Deliberately does not
/// accept the embedded-IPv4 IPv6 form (`::ffff:192.0.2.1`): no host in this
/// crate's test fixtures needs it, and rejecting it outright is safe (such a
/// host string simply never matches a certificate's `dNSName` either).
fn parse_ip_literal(host: &str) -> Option<IpLiteral> {
    if let Some(v4) = parse_ipv4(host) {
        return Some(IpLiteral::V4(v4));
    }
    if host.contains(':')
        && let Some(v6) = parse_ipv6(host)
    {
        return Some(IpLiteral::V6(v6));
    }
    None
}

fn normalize_dns_name(host: &str) -> Option<String> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > 253 || !host.is_ascii() {
        return None;
    }
    if host.split('.').any(str::is_empty) {
        return None;
    }
    Some(host.chars().map(|c| c.to_ascii_lowercase()).collect())
}

fn normalize_pattern(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || !bytes.is_ascii() {
        return None;
    }
    // `bytes.is_ascii()` guarantees this is valid UTF-8 too.
    let s = core::str::from_utf8(bytes).ok()?;
    let s = s.strip_suffix('.').unwrap_or(s);
    if s.is_empty() || s.split('.').any(str::is_empty) {
        return None;
    }
    Some(s.chars().map(|c| c.to_ascii_lowercase()).collect())
}

fn dns_pattern_matches(pattern_bytes: &[u8], host_norm: &str) -> bool {
    let Some(pattern) = normalize_pattern(pattern_bytes) else {
        return false;
    };
    let pat_labels: Vec<&str> = pattern.split('.').collect();
    let host_labels: Vec<&str> = host_norm.split('.').collect();

    if pat_labels.first() == Some(&"*") {
        // RFC 6125 section 6.4.3 rule 1, plus the brief's stricter minimum:
        // at least two labels must remain after the wildcard (never a
        // "*.com"-style two-label pattern), and the wildcard stands for
        // exactly one non-empty label.
        if pat_labels.len() < 3 || pat_labels.len() != host_labels.len() {
            return false;
        }
        pat_labels[1..] == host_labels[1..]
    } else {
        pattern == host_norm
    }
}

fn ip_matches(literal: &IpLiteral, san_bytes: &[u8]) -> bool {
    literal.as_bytes() == san_bytes
}

/// Whether `san` identifies `host`, per RFC 6125 and the rules in this
/// module's doc comment. `host` may be a DNS name or an IPv4/IPv6 literal.
pub fn matches_host(san: &SubjectAltName<'_>, host: &str) -> bool {
    if let Some(literal) = parse_ip_literal(host) {
        return san.names.iter().any(|n| matches!(n, GeneralName::Ip(bytes) if ip_matches(&literal, bytes)));
    }
    let Some(host_norm) = normalize_dns_name(host) else {
        return false;
    };
    san.names.iter().any(|n| match n {
        GeneralName::Dns(pattern) => dns_pattern_matches(pattern, &host_norm),
        GeneralName::Ip(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn san(names: Vec<GeneralName<'_>>) -> SubjectAltName<'_> {
        SubjectAltName { names }
    }

    #[test]
    fn exact_dns_match_is_case_insensitive() {
        let s = san(alloc::vec![GeneralName::Dns(b"Example.COM")]);
        assert!(matches_host(&s, "example.com"));
        assert!(matches_host(&s, "EXAMPLE.COM"));
        assert!(!matches_host(&s, "www.example.com"));
    }

    #[test]
    fn wildcard_matches_exactly_one_label() {
        let s = san(alloc::vec![GeneralName::Dns(b"*.example.com")]);
        assert!(matches_host(&s, "www.example.com"));
        assert!(matches_host(&s, "api.example.com"));
        assert!(!matches_host(&s, "example.com")); // wildcard needs a label to stand for
        assert!(!matches_host(&s, "a.b.example.com")); // wildcard is not multi-label
    }

    #[test]
    fn two_label_wildcard_never_matches() {
        let s = san(alloc::vec![GeneralName::Dns(b"*.com")]);
        assert!(!matches_host(&s, "example.com"));
        assert!(!matches_host(&s, "com"));
    }

    #[test]
    fn wildcard_must_be_the_entire_leftmost_label() {
        let s = san(alloc::vec![GeneralName::Dns(b"foo*.example.com")]);
        assert!(!matches_host(&s, "foobar.example.com"));
        let s2 = san(alloc::vec![GeneralName::Dns(b"api.*.example.com")]);
        assert!(!matches_host(&s2, "api.x.example.com"));
    }

    #[test]
    fn no_subject_alt_name_entries_never_match() {
        let s = san(Vec::new());
        assert!(!matches_host(&s, "example.com"));
    }

    #[test]
    fn ip_literal_matches_only_ip_sans() {
        let s = san(alloc::vec![GeneralName::Ip(&[127, 0, 0, 1]), GeneralName::Dns(b"example.com")]);
        assert!(matches_host(&s, "127.0.0.1"));
        assert!(!matches_host(&s, "127.0.0.2"));
        assert!(matches_host(&s, "example.com"));
    }

    #[test]
    fn ipv4_rejects_leading_zeros_and_out_of_range() {
        assert!(parse_ipv4("192.168.001.1").is_none());
        assert!(parse_ipv4("256.0.0.1").is_none());
        assert!(parse_ipv4("1.2.3").is_none());
        assert!(parse_ipv4("1.2.3.4.5").is_none());
        assert_eq!(parse_ipv4("192.168.1.1"), Some([192, 168, 1, 1]));
    }

    #[test]
    fn ipv6_literal_matches_ip_san() {
        let bytes: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let s = san(alloc::vec![GeneralName::Ip(&bytes)]);
        assert!(matches_host(&s, "::1"));
        assert!(!matches_host(&s, "::2"));
    }

    #[test]
    fn ipv6_full_form_parses() {
        let expected: [u8; 16] =
            [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        assert_eq!(parse_ipv6("2001:0db8:0000:0000:0000:0000:0000:0001"), Some(expected));
        assert_eq!(parse_ipv6("2001:db8::1"), Some(expected));
    }

    #[test]
    fn ipv6_rejects_double_compression() {
        assert!(parse_ipv6("2001::db8::1").is_none());
    }

    #[test]
    fn dns_name_with_trailing_dot_matches() {
        let s = san(alloc::vec![GeneralName::Dns(b"example.com")]);
        assert!(matches_host(&s, "example.com."));
    }

    #[test]
    fn non_ascii_host_never_matches() {
        let s = san(alloc::vec![GeneralName::Dns("café.example.com".as_bytes())]);
        assert!(!matches_host(&s, "café.example.com"));
    }
}
