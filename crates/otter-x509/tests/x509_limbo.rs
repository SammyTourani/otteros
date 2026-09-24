//! Runs the `webpki::`/`rfc5280::` `SERVER`-validation cases from
//! x509-limbo (<https://github.com/C2SP/x509-limbo>, C2SP/Google, converted
//! by `scripts/fetch-x509-limbo.py`) that apply to a TLS server-auth
//! client, per brief M8-T4. Every case not listed in
//! [`KNOWN_UNSUPPORTED`] must match its expected outcome; every ID in that
//! list corresponds to a documented, out-of-scope feature -- see this
//! crate's `README.md` "x509-limbo" section, which lists exactly the same
//! IDs grouped by cause.

mod common;

use std::collections::HashSet;
use std::path::PathBuf;

use otter_x509::{TrustStore, verify_server_chain};

/// x509-limbo cases whose expected outcome this crate's documented scope
/// cannot match, grouped by cause -- kept in the same grouping and order as
/// `README.md`'s "x509-limbo" section so the two stay easy to compare.
const KNOWN_UNSUPPORTED: &[&str] = &[
    // (`rfc5280::nc::permitted-dn-match` -- a directoryName nameConstraints
    // case this crate's dNSName-only support would also mismatch -- has no
    // `expected_peer_name` at all and is excluded from the fixture entirely
    // by scripts/fetch-x509-limbo.py, so it never reaches this list.)
    //
    // EKU: this crate treats `anyExtendedKeyUsage` as satisfying a
    // `serverAuth` requirement (a literal reading of RFC 5280 section
    // 4.2.1.12) and does not require `extKeyUsage` to be present at all
    // (RFC 5280 baseline: absence means no restriction) or consult a
    // parent certificate's own `extKeyUsage`. x509-limbo's `pedantic-
    // webpki-eku` cases test a stricter, webpki-crate-specific policy.
    "webpki::eku::ee-anyeku",
    "webpki::eku::ee-critical-eku",
    "webpki::eku::ee-without-eku",
    "webpki::eku::root-has-eku",
    // Wildcards: the brief's rule is "reject a literal two-label pattern
    // (`*.tld`)", not a full Public Suffix List consultation, so a
    // multi-label public suffix or private namespace is not rejected.
    "webpki::san::public-suffix-multi-label-wildcard-san",
    "webpki::san::public-suffix-private-namespace-wildcard-san",
    // Serial numbers: RFC 5280 does not make either of these a hard
    // requirement (the 20-octet length is explicitly non-normative
    // guidance, and 0 is not forbidden by the ASN.1 or the prose), so
    // this crate accepts them.
    "rfc5280::serial::too-long",
    "rfc5280::serial::zero",
    // Subscriber key restrictions: this crate has no notion of a
    // "forbidden but structurally valid" key -- an RSA modulus whose bit
    // length is merely not a multiple of 8 is still 2048-4096 bits and a
    // valid modulus, so `otter_crypto::rsa::PublicKey::from_components`
    // accepts it (P-192, DSA and genuinely-too-small RSA keys are all
    // already rejected as unsupported/invalid, matching x509-limbo here).
    "webpki::forbidden-rsa-key-not-divisible-by-8-in-leaf",
    "webpki::forbidden-rsa-not-divisible-by-8-in-root",
    // policyConstraints (2.5.29.36) is not implemented at all (outside
    // brief M8-T4's extension list); a non-critical, unrecognized
    // extension is -- correctly, per RFC 5280 section 4.2 -- ignored
    // rather than rejected.
    "rfc5280::pc::ica-noncritical-pc",
    // authorityKeyIdentifier: used only as an optional path-building filter
    // (brief M8-T4's design). This crate does not require it to be present
    // on non-self-issued certificates (RFC 5280 section 4.2.1.1's "MUST"),
    // require it to be non-critical, or validate a trust anchor's own AKI
    // for internal self-consistency (keyIdentifier vs. its own SKI, or the
    // authorityCertIssuer/authorityCertSerialNumber fields).
    "rfc5280::aki::critical-aki",
    "rfc5280::aki::leaf-missing-aki",
    "rfc5280::aki::intermediate-missing-aki",
    "rfc5280::aki::cross-signed-root-missing-aki",
    "webpki::aki::root-with-aki-missing-keyidentifier",
    "webpki::aki::root-with-aki-authoritycertissuer",
    "webpki::aki::root-with-aki-authoritycertserialnumber",
    "webpki::aki::root-with-aki-all-fields",
    "webpki::aki::root-with-aki-ski-mismatch",
    // subjectKeyIdentifier: likewise only an optional filter; presence is
    // not required (RFC 5280 section 4.2.1.2 also phrases it as a MUST for
    // CA certificates, which this crate does not enforce).
    "rfc5280::ski::root-missing-ski",
    "rfc5280::ski::intermediate-missing-ski",
    // nameConstraints, forms and placements beyond the brief's scope:
    // - a non-dNSName subtree (iPAddress, otherName, or a base of a type
    //   simply absent from the constrained certificates' own names) always
    //   makes the path invalid, per the brief's explicit rule, even where
    //   RFC 5280 would only require ignoring an inapplicable constraint;
    // - self-issued certificates are not exempted from having their own
    //   names checked against an ancestor's constraints (RFC 5280 section
    //   4.2.1.10 exempts them, except as the final certificate in the path);
    // - this crate does not specifically reject nameConstraints appearing
    //   on an end-entity certificate (structurally meaningless there, but
    //   simply never consulted, rather than being treated as an error);
    // - a dNSName SAN entry's own syntax (e.g. a leading '.', or whether a
    //   wildcard could match names a constraint would otherwise forbid) is
    //   not validated beyond what host-name matching itself requires.
    "rfc5280::nc::permitted-ipv4-match",
    "rfc5280::nc::permitted-ipv6-match",
    "rfc5280::nc::permitted-self-issued",
    "rfc5280::nc::permitted-different-constraint-type",
    "rfc5280::nc::excluded-different-constraint-type",
    "rfc5280::nc::not-allowed-in-ee-critical",
    "rfc5280::nc::nc-permits-invalid-dns-san",
    "rfc5280::nc::nc-forbids-othername-noop",
    "rfc5280::nc::nc-forbids-dnsname-wildcard-san",
    // This crate follows the strict RFC 5280 reading that nameConstraints
    // MUST be critical (section 4.2.1.10) and rejects a non-critical one
    // outright; x509-limbo's `webpki::` namespace tests the more lenient,
    // browser-compatible policy of enforcing it anyway. The two namespaces'
    // test cases for this exact scenario expect opposite outcomes by
    // design (see `rfc5280::nc::permitted-dns-match-noncritical`, which
    // *does* match this crate's behaviour) -- only one policy can be
    // implemented, and this crate picked the literal RFC 5280 text.
    "webpki::nc::permitted-dns-match-noncritical",
    // subjectAltName / empty-subject interaction (RFC 5280 section 4.2.1.6
    // requires SAN to be critical exactly when the subject is empty, and
    // CABF forbids critical SAN otherwise): this crate does not track a
    // parsed extension's own criticality flag past the point of dispatch,
    // so it cannot cross-check SAN's criticality against subject emptiness.
    "rfc5280::san::noncritical-with-empty-subject",
    "webpki::san::san-critical-with-nonempty-subject",
    // This crate does not enforce RFC 1034/1123 "preferred name syntax"
    // (e.g. rejecting an underscore) on subjectAltName dNSName entries
    // beyond what RFC 6125 host-name matching itself requires.
    "rfc5280::san::underscore-dns",
    // The brief requires a TLS leaf not be a CA certificate (matching
    // common "webpki"-style policy); bare RFC 5280 does not forbid this,
    // which is what this baseline-RFC-5280-namespaced case tests.
    "rfc5280::ca-as-leaf",
    // CABF Baseline Requirements 7.1.4.3 (commonName/subjectAltName
    // consistency) is not implemented: per the brief's explicit scope,
    // the deprecated commonName attribute is never consulted for host-name
    // verification at all, including as a structural check unrelated to
    // the actual host being verified.
    "webpki::cn::ipv4-hex-mismatch",
    "webpki::cn::ipv4-leading-zeros-mismatch",
    "webpki::cn::ipv6-uppercase-mismatch",
    "webpki::cn::ipv6-uncompressed-mismatch",
    "webpki::cn::ipv6-non-rfc5952-mismatch",
    "webpki::cn::punycode-not-in-san",
    "webpki::cn::utf8-vs-punycode-mismatch",
    "webpki::cn::not-in-san",
    "webpki::cn::case-mismatch",
    // This crate does not parse the internal structure of extensions it
    // does not implement (authorityInfoAccess is not in the brief's
    // extension list); a non-critical, malformed AIA is therefore ignored
    // like any other unrecognized non-critical extension (RFC 5280
    // section 4.2).
    "webpki::malformed-aia",
];

struct Case {
    id: String,
    expect_success: bool,
    validation_time: u64,
    host: String,
    trusted: Vec<Vec<u8>>,
    untrusted: Vec<Vec<u8>>,
    leaf: Vec<u8>,
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex fixture")).collect()
}

fn hex_list_decode(s: &str) -> Vec<Vec<u8>> {
    if s.is_empty() { Vec::new() } else { s.split(',').map(hex_decode).collect() }
}

fn cases_path() -> PathBuf {
    common::fixtures_dir().join("x509_limbo").join("cases.txt")
}

fn load_cases() -> Vec<Case> {
    let path = cases_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {path:?} ({e}); run scripts/fetch-x509-limbo.py first"));
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let fields: Vec<&str> = line.split('|').collect();
            let [id, expected_result, validation_time, _host_kind, host_value, trusted, untrusted, leaf] =
                fields.as_slice()
            else {
                panic!("malformed x509-limbo fixture line ({} fields): {line}", fields.len());
            };
            Case {
                id: (*id).to_string(),
                expect_success: *expected_result == "SUCCESS",
                validation_time: validation_time.parse().expect("valid unix time"),
                host: (*host_value).to_string(),
                trusted: hex_list_decode(trusted),
                untrusted: hex_list_decode(untrusted),
                leaf: hex_decode(leaf),
            }
        })
        .collect()
}

#[test]
fn x509_limbo_applicable_cases() {
    let cases = load_cases();
    assert!(cases.len() >= 100, "expected a substantial number of applicable cases, got {}", cases.len());

    let known_unsupported: HashSet<&str> = KNOWN_UNSUPPORTED.iter().copied().collect();
    let mut matched = 0usize;
    let mut documented_mismatches = 0usize;
    let mut undocumented: Vec<String> = Vec::new();
    let mut stale_known_unsupported: Vec<&str> = Vec::new();

    for case in &cases {
        let trusted_refs: Vec<&[u8]> = case.trusted.iter().map(Vec::as_slice).collect();
        let roots = TrustStore::from_der_roots(&trusted_refs);

        let mut chain: Vec<&[u8]> = Vec::with_capacity(1 + case.untrusted.len());
        chain.push(&case.leaf);
        chain.extend(case.untrusted.iter().map(Vec::as_slice));

        let outcome = verify_server_chain(&chain, &case.host, case.validation_time, &roots);
        let matches_expectation = outcome.is_ok() == case.expect_success;

        if matches_expectation {
            matched += 1;
            if known_unsupported.contains(case.id.as_str()) {
                stale_known_unsupported.push(&case.id);
            }
        } else if known_unsupported.contains(case.id.as_str()) {
            documented_mismatches += 1;
        } else {
            undocumented.push(format!(
                "{} (expected_success={}, got_ok={}, err={:?})",
                case.id,
                case.expect_success,
                outcome.is_ok(),
                outcome.err()
            ));
        }
    }

    println!(
        "x509-limbo: {matched}/{} applicable cases matched expectations ({documented_mismatches} documented unsupported)",
        cases.len()
    );
    assert!(undocumented.is_empty(), "undocumented x509-limbo mismatches (add to KNOWN_UNSUPPORTED and README.md, with a reason, or fix the bug):\n{undocumented:#?}");
    assert!(
        stale_known_unsupported.is_empty(),
        "these KNOWN_UNSUPPORTED cases now pass and should be removed from the list (and README.md): {stale_known_unsupported:?}"
    );
    assert_eq!(documented_mismatches, KNOWN_UNSUPPORTED.len(), "every KNOWN_UNSUPPORTED id should appear exactly once among the applicable cases");
}
