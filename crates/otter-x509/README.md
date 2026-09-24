# otter-x509

OtterOS's own X.509, written from scratch: a strict, minimal DER reader,
RFC 5280 certificate parsing, certification path building and validation
(with backtracking), RFC 6125 host-name matching, and an embedded
Mozilla-derived trust anchor bundle. `#![no_std]` + `alloc`, zero external
dependencies except `otter-crypto` by path (DECISIONS.md D2, D22, D27) --
this crate calls into `otter-crypto` only for hashing and signature
*verification* (it never holds or uses a private key), proven against real
test-suite data (x509-limbo) and six real-world hosts' captured certificate
chains before any network code uses it.

## Layout

| Module | What it does |
| --- | --- |
| `der` | Strict, minimal DER reader: definite lengths only, minimal INTEGER/length/OID encoding, clean BIT STRING padding, bounded nesting depth |
| `oid` | Object identifiers this crate recognizes, as raw DER content bytes |
| `time` | UTCTime/GeneralizedTime (RFC 5280 4.1.2.5) to Unix seconds |
| `name` | `Name` (RDN sequence): raw bytes for exact comparison, parsed attributes for display and a normalized-comparison fallback |
| `spki` | `SubjectPublicKeyInfo` -> an `otter-crypto` RSA/P-256/P-384 public key |
| `extensions` | X.509v3 extensions: basicConstraints, keyUsage, extKeyUsage, subjectAltName, authority/subjectKeyIdentifier, nameConstraints |
| `sigalg` | `AlgorithmIdentifier` -> a signature scheme + hash (including RSASSA-PSS parameters), and the one call into `otter-crypto` to verify a signature |
| `cert` | `Certificate`: parses a whole DER certificate into the above, enforcing RFC 5280 structural rules beyond plain DER |
| `hostname` | RFC 6125 host-name matching against `subjectAltName` (`dNSName`/`iPAddress` only) |
| `path` | Path building (depth-first search with backtracking) and RFC 5280 chain validation; `TrustStore`, `VerifiedChain` and the public `verify_server_chain` entry point |
| `roots` | The embedded Mozilla-derived (curl `cacert.pem`) trust anchors |
| `error` | The one `X509Error` every fallible operation in this crate returns |

## API

```rust
otter_x509::verify_server_chain(
    chain_der: &[&[u8]],   // leaf first, then the server's other certificates, any order, extras/duplicates allowed
    host: &str,             // DNS name or IPv4/IPv6 literal
    now_unix: u64,
    roots: &otter_x509::TrustStore,
) -> Result<otter_x509::VerifiedChain, otter_x509::X509Error>
```

`VerifiedChain::leaf_public_key()` returns the leaf's `otter-crypto` public
key (for a TLS 1.3 `CertificateVerify`); `leaf_subject()`/`leaf_issuer()`
return display strings for a UI. `TrustStore::from_der_roots` builds a store
from arbitrary DER roots (e.g. a test root); `otter_x509::roots::trust_store()`
builds one from the embedded Mozilla-derived bundle.

## Testing

```sh
cd crates
cargo test -p otter-x509                                             # unit + integration tests (host, aarch64)
cargo clippy -p otter-x509 --tests -- -D warnings                     # lint
cargo build -p otter-x509 --target x86_64-unknown-none                # proves it's genuinely no_std
```

Fixtures need generating/fetching first (each is committed once generated,
so this is a one-time -- or re-run-when-the-real-world-changes -- step, not
part of every `cargo test`):

```sh
../scripts/fetch-roots.py         # -> src/roots/generated.rs (curl's cacert.pem, SHA-256-pinned)
../scripts/make-test-pki.sh       # -> tests/fixtures/pki/*.der (+ .key)
../scripts/capture-chains.py      # -> tests/fixtures/chains/*.der + manifest.txt (6 real hosts, handshake only)
../scripts/fetch-x509-limbo.py    # -> tests/fixtures/x509_limbo/cases.txt (SHA-256-pinned)
```

- `tests/test_pki.rs` -- the positive cases (exact/wildcard/IP SANs, a
  name-constrained intermediate) and every negative case the brief calls
  for (wrong host, expired at a later time, missing intermediate,
  self-signed leaf, non-CA issuer, `pathLenConstraint` exceeded, EKU
  mismatch, name-constraint violation, unknown critical extension) against
  `scripts/make-test-pki.sh`'s from-scratch test PKI (root, an intermediate
  with `pathLen: 0`, a name-constrained intermediate, and leaves covering
  each case).
- `tests/captured_chains.rs` -- validates `scripts/capture-chains.py`'s
  point-in-time capture of `example.com`, `www.google.com`, `github.com`,
  `www.cloudflare.com`, `letsencrypt.org` and `api.anthropic.com`'s real
  certificate chains against the embedded Mozilla-derived root store, at
  the Unix time they were captured.
- `tests/x509_limbo.rs` -- see below.
- `tests/robustness.rs` -- flips every single byte, one at a time, of every
  fixture certificate above and confirms `verify_server_chain` never panics
  (only ever returns `Ok` or `Err`).

## x509-limbo

[x509-limbo](https://github.com/C2SP/x509-limbo) (C2SP/Google, Apache-2.0)
is an extensive, adversarial X.509 path-validation test suite covering
RFC 5280 to the letter, the stricter "webpki" (browser-compatible) profile,
and CA/Browser Forum Baseline Requirements, well beyond what a minimal TLS
client strictly needs. `scripts/fetch-x509-limbo.py` converts the
`validation_kind == "SERVER"` cases whose `id` starts with `webpki::` or
`rfc5280::` -- the two namespaces relevant to "a TLS server-auth client",
per the brief -- into `tests/fixtures/x509_limbo/cases.txt` (154 cases; one
more, `rfc5280::nc::permitted-dn-match`, has no `expected_peer_name` at all,
which `verify_server_chain` always requires, so it is excluded rather than
mismatched).

**108 of 154 applicable cases (70%) match this crate's expected outcome
exactly.** The other 46 are a fixed, named list
(`x509_limbo.rs::KNOWN_UNSUPPORTED`) the test asserts against: every one of
them is either a feature genuinely outside brief M8-T4's scope, or a case
where x509-limbo's `rfc5280::` and `webpki::` namespaces test two opposite,
mutually exclusive policies for the same input and this crate can only ever
match one. None reflect an unnoticed parsing or cryptographic bug -- see
`tests/x509_limbo.rs` for per-case reasoning; summarized by cause:

- **`authorityKeyIdentifier`/`subjectKeyIdentifier`** (16 cases: 4
  `rfc5280::aki::*`, 5 `webpki::aki::*`, 2 `rfc5280::ski::*`, plus AKI/SKI
  mismatches folded into other groups below): used only as an optional
  path-building filter (brief M8-T4's design, "candidates by name and
  AKI/SKI when present"). Presence is not required on non-self-issued
  certificates, criticality is not checked, and a trust anchor's own AKI is
  never validated for internal self-consistency.
- **`nameConstraints` forms and placements beyond brief scope** (9 cases,
  `rfc5280::nc::*`): a non-`dNSName` subtree (`iPAddress`, `otherName`, or
  any base of a type simply absent from the constrained certificates' own
  names) makes the whole path invalid, per the brief's explicit rule --
  even where RFC 5280 would only require ignoring an inapplicable
  constraint. Self-issued certificates are not exempted from constraint
  checking (RFC 5280 section 4.2.1.10 exempts them). `nameConstraints` is
  not specifically rejected when it appears on an end-entity certificate.
  A `dNSName` SAN entry's own syntax and a wildcard's full matching scope
  are not validated beyond what RFC 6125 host-name matching itself needs.
- **`nameConstraints` criticality: two opposite policies** (2 cases): this
  crate follows the literal RFC 5280 text ("conforming CAs MUST mark this
  extension as critical", section 4.2.1.10) and rejects a non-critical
  `nameConstraints` outright -- matching `rfc5280::nc::permitted-dns-match-
  noncritical`'s expected *failure*. x509-limbo's `webpki::` namespace
  separately tests the more lenient, browser-compatible policy of
  enforcing a non-critical `nameConstraints` anyway
  (`webpki::nc::permitted-dns-match-noncritical`, expected *success*): the
  same certificate, opposite expected outcomes by design. Only one policy
  can be implemented.
- **`extKeyUsage` policy** (4 cases, `webpki::eku::*`, all tagged
  `pedantic-webpki-eku`): this crate treats `anyExtendedKeyUsage` as
  satisfying a `serverAuth` requirement (a literal reading of RFC 5280
  section 4.2.1.12), does not require `extKeyUsage` to be present at all
  (RFC 5280 baseline: absence means no restriction), and never consults a
  parent certificate's own `extKeyUsage`.
- **Wildcard scope: no Public Suffix List** (2 cases, tagged
  `pedantic-public-suffix-wildcard`): the brief's rule is "reject a literal
  two-label pattern (`*.tld`)", not a full Public Suffix List consultation,
  so a multi-label public suffix (`*.co.uk`) or private namespace is not
  rejected.
- **Serial numbers** (2 cases, tagged `pedantic-serial-number`): RFC 5280
  does not make either a hard requirement (the 20-octet length is
  explicitly non-normative guidance, and `0` is not forbidden), so this
  crate accepts a zero or over-long serial number.
- **Subscriber key restrictions** (2 cases, tagged
  `pedantic-webpki-subscriber-key`): an RSA modulus whose bit length is
  merely not a multiple of 8 is still a valid 2048-4096-bit modulus to this
  crate (P-192, DSA and genuinely-too-small RSA keys are all already
  rejected, matching x509-limbo).
- **`policyConstraints` not implemented** (1 case): outside brief M8-T4's
  extension list entirely; a non-critical, unrecognized extension is --
  correctly, per RFC 5280 section 4.2 -- ignored rather than rejected.
- **CA-as-leaf, bare RFC 5280** (1 case, `rfc5280::ca-as-leaf`): the brief
  requires a TLS leaf not be a CA certificate (common "webpki"-style
  policy); bare RFC 5280 does not forbid it, which is what this
  baseline-namespaced case tests.
- **CABF commonName/subjectAltName consistency not implemented** (9 cases,
  `webpki::cn::*`): CA/Browser Forum Baseline Requirements 7.1.4.3 ties the
  deprecated `commonName` attribute to `subjectAltName`. Per the brief's
  explicit scope ("no CN fallback"), this crate never consults `commonName`
  for host verification at all, including as a structural check unrelated
  to the host actually being verified.
- **`subjectAltName` criticality vs. empty subject** (2 cases): RFC 5280
  section 4.2.1.6 requires `subjectAltName` to be critical exactly when the
  subject is empty, and CABF forbids critical SAN otherwise; this crate
  does not track a parsed extension's own criticality flag past the point
  of dispatch, so it cannot cross-check SAN's criticality against subject
  emptiness.
- **`dNSName` "preferred name syntax"** (1 case): RFC 1034/1123 label
  rules (e.g. rejecting an underscore) are not enforced beyond what
  RFC 6125 host-name matching itself requires.
- **Unrecognized-extension internal structure** (1 case,
  `webpki::malformed-aia`): this crate does not parse the internal
  structure of extensions it does not implement at all
  (`authorityInfoAccess` is not in the brief's extension list); a
  non-critical, malformed one is ignored like any other unrecognized
  non-critical extension.

## Third-party data

See `THIRD_PARTY_NOTICES.md` at the repository root: the embedded root
bundle (curl project's `cacert.pem`, Mozilla-derived, MPL-2.0) and the
x509-limbo test suite (C2SP/Google, Apache-2.0) are both data, fetched by
the `scripts/` above, not code.
