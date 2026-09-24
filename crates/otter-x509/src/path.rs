//! Certification path building and validation (RFC 5280 section 6), and the
//! crate's one public entry point, [`verify_server_chain`].
//!
//! Path *building* here is a depth-first search with full backtracking: at
//! each step, every candidate issuer -- first the embedded/caller-supplied
//! trust anchors, then the server's own untrusted certificates, in any
//! order and possibly duplicated -- is tried; a candidate that fails any
//! RFC 5280 constraint (bad signature, not a CA, `pathLenConstraint`
//! exceeded, `keyUsage` missing `keyCertSign`, an unsupported or violated
//! `nameConstraints`) is simply skipped in favour of the next one, and a
//! dead end backtracks to try an alternative earlier in the path, exactly as
//! the brief specifies. A `used` bitmap over the untrusted pool guarantees
//! termination (loop detection): the search can recurse at most once per
//! pool certificate.
//!
//! [`TrustStore`] and [`VerifiedChain`] both own their data outright (no
//! lifetime tied to the certificates that produced them): a `TrustStore` is
//! built once (typically from [`crate::roots`]'s embedded Mozilla-derived
//! bundle) and reused across every handshake, and a `VerifiedChain` is
//! meant to outlive the DER bytes `verify_server_chain` was called with.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::cert::Certificate;
use crate::error::X509Error;
use crate::extensions::{GeneralName, NameConstraints};
use crate::hostname;
use crate::name::Name;
use crate::sigalg;
pub use crate::spki::PublicKey;

/// How many certificates a chain may contain at most, leaf included --
/// generous for any real TLS deployment, but a fixed bound so a hostile
/// server cannot force unbounded parsing/search work.
const MAX_CHAIN_CERTS: usize = 32;

/// How many intermediate CA certificates a built path may contain at most
/// (the brief's own "depth limit 8"), not counting the leaf or the trust
/// anchor.
const MAX_PATH_DEPTH: usize = 8;

/// One embedded or caller-supplied trust anchor: enough of a CA certificate
/// to match it by subject name (and, as an optional filter, key
/// identifier), verify a signature under its public key, and honour its own
/// `basicConstraints`/`keyUsage`/`nameConstraints`/validity period.
struct TrustAnchor {
    /// The anchor's `subject`, as raw `RDNSequence` content bytes.
    subject: Vec<u8>,
    public_key: PublicKey,
    /// The anchor's own validity period (Unix seconds).
    not_before: i64,
    not_after: i64,
    /// `basicConstraints.pathLenConstraint`, if present. Every stored anchor
    /// already satisfies `cA: TRUE` (checked, and required, when the
    /// `TrustStore` was built) so there is no separate `is_ca` flag to check
    /// again here.
    path_len: Option<u64>,
    /// `Some(false)` if `keyUsage` was present without `keyCertSign`
    /// (disqualifying this anchor as an issuer); `Some(true)` or `None`
    /// (absent `keyUsage`, no restriction) both mean "may sign".
    key_cert_sign_ok: Option<bool>,
    /// `subjectKeyIdentifier`, if present, for the same optional AKI/SKI
    /// candidate-narrowing [`crate::cert::Certificate`]'s pool candidates use.
    skid: Option<Vec<u8>>,
    name_constraints_owned: Option<OwnedNameConstraints>,
}

/// An owned copy of the `dNSName`-subtree facts from a trust anchor's
/// `nameConstraints` extension (a [`TrustAnchor`] cannot borrow from the
/// certificate it was built from -- see this module's doc comment).
struct OwnedNameConstraints {
    permitted_dns: Vec<String>,
    excluded_dns: Vec<String>,
    unsupported: bool,
}

/// A set of trust anchors to validate certification paths against: either
/// [`crate::roots::trust_store`]'s embedded Mozilla-derived bundle, or a
/// custom set (e.g. a test root) built with [`TrustStore::from_der_roots`].
/// Build once, reuse across every handshake.
pub struct TrustStore {
    anchors: Vec<TrustAnchor>,
}

impl TrustStore {
    /// An empty trust store (trusts nothing).
    pub fn empty() -> TrustStore {
        TrustStore { anchors: Vec::new() }
    }

    /// Builds a trust store from a set of DER-encoded root certificates.
    /// Each is parsed with the same strict certificate parser as any other
    /// certificate; a root that fails to parse, or parses but does not
    /// assert `basicConstraints.cA: TRUE`, is skipped rather than making the
    /// whole store unusable (the Mozilla bundle this crate embeds is large
    /// and heterogeneous; one unusual legacy root should not take down every
    /// other one). Use [`TrustStore::len`] if a caller needs to confirm how
    /// many roots actually loaded (e.g. in tests, to catch a fixture that
    /// silently failed to parse at all).
    pub fn from_der_roots(roots_der: &[&[u8]]) -> TrustStore {
        let mut anchors = Vec::with_capacity(roots_der.len());
        for der in roots_der {
            let Ok(cert) = Certificate::parse(der) else { continue };
            if !cert.is_ca() {
                continue;
            }
            let subject = cert.subject.raw().to_vec();
            let path_len = cert.basic_constraints.and_then(|bc| bc.path_len);
            let key_cert_sign_ok = cert.key_usage.map(|ku| ku.key_cert_sign());
            let skid = cert.skid.map(<[u8]>::to_vec);
            let name_constraints_owned = cert.name_constraints.as_ref().map(own_name_constraints);
            let (not_before, not_after) = (cert.not_before, cert.not_after);
            let public_key = cert.public_key;
            anchors.push(TrustAnchor {
                subject,
                public_key,
                not_before,
                not_after,
                path_len,
                key_cert_sign_ok,
                skid,
                name_constraints_owned,
            });
        }
        TrustStore { anchors }
    }

    /// How many trust anchors successfully loaded.
    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    /// Whether this store has no usable trust anchors.
    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }
}

fn own_name_constraints(nc: &NameConstraints<'_>) -> OwnedNameConstraints {
    OwnedNameConstraints {
        permitted_dns: nc.permitted_dns.iter().filter_map(|b| ascii_lower(b)).collect(),
        excluded_dns: nc.excluded_dns.iter().filter_map(|b| ascii_lower(b)).collect(),
        unsupported: nc.unsupported,
    }
}

fn ascii_lower(bytes: &[u8]) -> Option<String> {
    if !bytes.is_ascii() {
        return None;
    }
    Some(bytes.iter().map(|&b| (b as char).to_ascii_lowercase()).collect())
}

/// The result of successfully verifying a server's certificate chain: the
/// leaf's public key (to verify a TLS 1.3 `CertificateVerify` against) and
/// human-readable names for a UI. Owns its data outright.
pub struct VerifiedChain {
    leaf_public_key: PublicKey,
    leaf_subject: String,
    leaf_issuer: String,
}

impl VerifiedChain {
    /// The leaf certificate's public key.
    pub fn leaf_public_key(&self) -> &PublicKey {
        &self.leaf_public_key
    }

    /// The leaf certificate's subject, as a display string.
    pub fn leaf_subject(&self) -> &str {
        &self.leaf_subject
    }

    /// The leaf certificate's issuer, as a display string.
    pub fn leaf_issuer(&self) -> &str {
        &self.leaf_issuer
    }
}

/// Decides, like a TLS client, whether `chain_der` (the leaf certificate
/// first, then the server's other certificates in any order, extras and
/// duplicates allowed) is a trustworthy certificate chain for `host` at
/// `now_unix` (Unix seconds), anchored in `roots`.
pub fn verify_server_chain(
    chain_der: &[&[u8]],
    host: &str,
    now_unix: u64,
    roots: &TrustStore,
) -> Result<VerifiedChain, X509Error> {
    if chain_der.is_empty() {
        return Err(X509Error::EmptyChain);
    }
    if chain_der.len() > MAX_CHAIN_CERTS {
        return Err(X509Error::ChainTooLong);
    }

    let leaf = Certificate::parse(chain_der[0])?;
    // A pool certificate that fails to parse is simply not a usable
    // candidate (skipped, like a name mismatch or a bad signature would
    // be), rather than aborting the whole call: a server sending one
    // malformed or irrelevant extra certificate alongside an otherwise
    // valid chain should not sink a path that never needed it.
    let pool: Vec<Certificate<'_>> = chain_der[1..].iter().filter_map(|der| Certificate::parse(der).ok()).collect();

    let now = i64::try_from(now_unix).unwrap_or(i64::MAX);
    if now < leaf.not_before {
        return Err(X509Error::CertificateNotYetValid);
    }
    if now > leaf.not_after {
        return Err(X509Error::CertificateExpired);
    }
    if leaf.is_ca() {
        return Err(X509Error::LeafIsCertificateAuthority);
    }
    if let Some(eku) = leaf.ext_key_usage
        && !eku.server_auth
    {
        return Err(X509Error::ExtendedKeyUsageMismatch);
    }

    {
        let mut used = vec![false; pool.len()];
        let mut chain_so_far: Vec<&Certificate<'_>> = vec![&leaf];
        find_path(&mut chain_so_far, &pool, &mut used, roots, now)?;
    }

    let san = leaf.san.as_ref().ok_or(X509Error::HostnameMismatch)?;
    if !hostname::matches_host(san, host) {
        return Err(X509Error::HostnameMismatch);
    }

    Ok(VerifiedChain {
        leaf_subject: leaf.subject.display(),
        leaf_issuer: leaf.issuer.display(),
        leaf_public_key: leaf.public_key,
    })
}

/// Depth-first search with backtracking for an issuer of
/// `chain_so_far.last()`, extending `chain_so_far` on success. `used` marks
/// which `pool` entries are already part of the path being tried (loop
/// detection: each recursive step consumes one previously-unused pool
/// entry, so the search terminates in at most `pool.len()` levels even
/// without [`MAX_PATH_DEPTH`]).
fn find_path<'a, 'p>(
    chain_so_far: &mut Vec<&'p Certificate<'a>>,
    pool: &'p [Certificate<'a>],
    used: &mut [bool],
    roots: &TrustStore,
    now: i64,
) -> Result<(), X509Error> {
    let current = *chain_so_far.last().expect("chain_so_far is never empty");
    let depth = chain_so_far.len() - 1; // intermediates already placed below `current`

    if depth >= MAX_PATH_DEPTH {
        return Err(X509Error::PathDepthExceeded);
    }

    for anchor in &roots.anchors {
        if !subject_matches_anchor(&anchor.subject, &current.issuer) {
            continue;
        }
        if let (Some(akid), Some(skid)) = (current.akid, anchor.skid.as_deref())
            && akid != skid
        {
            continue;
        }
        if let Some(pl) = anchor.path_len
            && (pl as usize) < depth
        {
            continue;
        }
        if anchor.key_cert_sign_ok == Some(false) {
            continue;
        }
        if now < anchor.not_before || now > anchor.not_after {
            continue;
        }
        if let Some(nc) = &anchor.name_constraints_owned
            && (nc.unsupported
                || !name_constraints_satisfied(&nc.permitted_dns, &nc.excluded_dns, chain_so_far.as_slice()))
        {
            continue;
        }
        if sigalg::verify_signature(&anchor.public_key, current.sig_alg, current.tbs_raw, current.signature).is_err() {
            continue;
        }
        return Ok(());
    }

    for i in 0..pool.len() {
        if used[i] {
            continue;
        }
        let candidate = &pool[i];
        if !candidate.subject.matches(&current.issuer) {
            continue;
        }
        if let (Some(akid), Some(skid)) = (current.akid, candidate.skid)
            && akid != skid
        {
            continue;
        }
        if !candidate.is_ca() {
            continue;
        }
        if let Some(bc) = candidate.basic_constraints
            && let Some(pl) = bc.path_len
            && (pl as usize) < depth
        {
            continue;
        }
        if let Some(ku) = candidate.key_usage
            && !ku.key_cert_sign()
        {
            continue;
        }
        if now < candidate.not_before || now > candidate.not_after {
            continue;
        }
        if let Some(nc) = &candidate.name_constraints {
            let owned = own_name_constraints(nc);
            if owned.unsupported
                || !name_constraints_satisfied(&owned.permitted_dns, &owned.excluded_dns, chain_so_far.as_slice())
            {
                continue;
            }
        }
        if sigalg::verify_signature(&candidate.public_key, current.sig_alg, current.tbs_raw, current.signature).is_err()
        {
            continue;
        }

        used[i] = true;
        chain_so_far.push(candidate);
        if find_path(chain_so_far, pool, used, roots, now).is_ok() {
            return Ok(());
        }
        chain_so_far.pop();
        used[i] = false;
    }

    Err(X509Error::NoPathFound)
}

fn subject_matches_anchor(anchor_subject: &[u8], issuer: &Name<'_>) -> bool {
    if anchor_subject == issuer.raw() {
        return true;
    }
    match Name::from_rdn_sequence_content(anchor_subject) {
        Ok(anchor_name) => anchor_name.matches(issuer),
        Err(_) => false,
    }
}

fn dns_in_subtree(name: &str, base: &str) -> bool {
    if base.is_empty() {
        return true;
    }
    if !name.ends_with(base) || name.len() < base.len() {
        return false;
    }
    name.len() == base.len() || name.as_bytes()[name.len() - base.len() - 1] == b'.'
}

fn dns_satisfies(name_norm: &str, permitted: &[String], excluded: &[String]) -> bool {
    if !permitted.is_empty() && !permitted.iter().any(|b| dns_in_subtree(name_norm, b)) {
        return false;
    }
    !excluded.iter().any(|b| dns_in_subtree(name_norm, b))
}

/// Checks every `dNSName` SAN entry of every certificate in `chain_so_far`
/// (all of them "below", i.e. closer to the leaf than, the certificate that
/// carries `nc`) against `nc`'s permitted/excluded subtrees. Shared by both
/// a pool candidate's borrowed [`NameConstraints`] (normalized on the spot)
/// and a [`TrustAnchor`]'s already-owned, pre-normalized copy.
fn name_constraints_satisfied(permitted: &[String], excluded: &[String], chain_so_far: &[&Certificate<'_>]) -> bool {
    for cert in chain_so_far {
        let Some(san) = &cert.san else { continue };
        for name in &san.names {
            if let GeneralName::Dns(bytes) = name {
                let Some(name_norm) = ascii_lower(bytes) else { continue };
                if !dns_satisfies(&name_norm, permitted, excluded) {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chain_is_rejected() {
        let roots = TrustStore::empty();
        assert_eq!(verify_server_chain(&[], "example.com", 0, &roots).err(), Some(X509Error::EmptyChain));
    }

    #[test]
    fn chain_too_long_is_rejected() {
        let roots = TrustStore::empty();
        let one = [0x30u8, 0x00];
        let refs: Vec<&[u8]> = core::iter::repeat_n(one.as_slice(), MAX_CHAIN_CERTS + 1).collect();
        assert_eq!(verify_server_chain(&refs, "example.com", 0, &roots).err(), Some(X509Error::ChainTooLong));
    }

    #[test]
    fn garbage_leaf_is_a_der_error_not_a_panic() {
        let roots = TrustStore::empty();
        let garbage = [0xffu8; 8];
        assert!(verify_server_chain(&[&garbage], "example.com", 0, &roots).is_err());
    }

    #[test]
    fn empty_trust_store_has_zero_anchors() {
        assert_eq!(TrustStore::empty().len(), 0);
        assert!(TrustStore::empty().is_empty());
    }

    #[test]
    fn dns_in_subtree_matches_exact_and_subdomain_not_suffix_collision() {
        assert!(dns_in_subtree("example.com", "example.com"));
        assert!(dns_in_subtree("www.example.com", "example.com"));
        assert!(!dns_in_subtree("evilexample.com", "example.com"));
        assert!(!dns_in_subtree("example.com", "www.example.com"));
        assert!(dns_in_subtree("anything.at.all", ""));
    }
}

