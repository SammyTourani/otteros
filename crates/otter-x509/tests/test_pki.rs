//! End-to-end tests against `scripts/make-test-pki.sh`'s fixtures (brief
//! M8-T4): the positive cases (exact/wildcard/IP SANs, a name-constrained
//! intermediate) and every negative case the brief calls for (wrong host,
//! expired at a later time, missing intermediate, self-signed leaf, non-CA
//! issuer, pathLen exceeded, EKU mismatch, name-constraint violation,
//! unknown critical extension).

mod common;

use common::{AFTER_ALL_EXPIRED, BEFORE_ALL_VALID, NOW_NORMAL, pki};
use otter_x509::{PublicKey, TrustStore, X509Error, verify_server_chain};

fn root_store() -> TrustStore {
    let root = pki("root");
    let store = TrustStore::from_der_roots(&[&root]);
    assert_eq!(store.len(), 1, "the test root itself failed to parse");
    store
}

// ---------------------------------------------------------------------
// Positive cases
// ---------------------------------------------------------------------

#[test]
fn exact_san_chain_verifies() {
    let leaf = pki("leaf_exact");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    let chain = verify_server_chain(&[&leaf, &int0], "leaf.otter-test.example", NOW_NORMAL, &roots)
        .expect("valid chain should verify");
    assert_eq!(chain.leaf_subject(), "CN=leaf.otter-test.example");
    assert_eq!(chain.leaf_issuer(), "CN=Otter Test Intermediate PathLen0");
    assert!(matches!(chain.leaf_public_key(), PublicKey::Rsa(_)));
}

#[test]
fn wildcard_san_matches_any_single_label() {
    let leaf = pki("leaf_wildcard");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    assert!(verify_server_chain(&[&leaf, &int0], "www.wild.otter-test.example", NOW_NORMAL, &roots).is_ok());
    assert!(verify_server_chain(&[&leaf, &int0], "api.wild.otter-test.example", NOW_NORMAL, &roots).is_ok());
    // Bare domain (no label for the wildcard to stand for) must not match.
    assert!(verify_server_chain(&[&leaf, &int0], "wild.otter-test.example", NOW_NORMAL, &roots).is_err());
}

#[test]
fn ip_literal_san_matches() {
    let leaf = pki("leaf_ip");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    assert!(verify_server_chain(&[&leaf, &int0], "203.0.113.42", NOW_NORMAL, &roots).is_ok());
    assert!(verify_server_chain(&[&leaf, &int0], "203.0.113.43", NOW_NORMAL, &roots).is_err());
}

#[test]
fn extra_and_duplicate_pool_certificates_are_tolerated() {
    let leaf = pki("leaf_exact");
    let int0 = pki("intermediate_pathlen0");
    let unrelated = pki("non_ca");
    let roots = root_store();
    // Pool order: an unrelated cert first, then two copies of the real
    // intermediate -- "extras and duplicates allowed", per the brief.
    let chain: [&[u8]; 4] = [&leaf, &unrelated, &int0, &int0];
    assert!(verify_server_chain(&chain, "leaf.otter-test.example", NOW_NORMAL, &roots).is_ok());
}

#[test]
fn name_constrained_intermediate_permits_matching_subdomain() {
    let leaf = pki("leaf_nc_ok");
    let int_nc = pki("intermediate_name_constrained");
    let roots = root_store();
    let chain = verify_server_chain(&[&leaf, &int_nc], "host.nc.otter-test.example", NOW_NORMAL, &roots)
        .expect("name-constrained chain within the permitted subtree should verify");
    assert!(matches!(chain.leaf_public_key(), PublicKey::Rsa(_)));
}

// ---------------------------------------------------------------------
// Negative cases (brief's required list)
// ---------------------------------------------------------------------

#[test]
fn wrong_host_is_rejected() {
    let leaf = pki("leaf_exact");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf, &int0], "not-the-right-host.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::HostnameMismatch)
    );
}

#[test]
fn expired_at_a_later_time_is_rejected() {
    let leaf = pki("leaf_exact");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    // Valid as of NOW_NORMAL; the same chain, checked after its notAfter, must fail.
    assert!(verify_server_chain(&[&leaf, &int0], "leaf.otter-test.example", NOW_NORMAL, &roots).is_ok());
    assert_eq!(
        verify_server_chain(&[&leaf, &int0], "leaf.otter-test.example", AFTER_ALL_EXPIRED, &roots).err(),
        Some(X509Error::CertificateExpired)
    );
}

#[test]
fn already_expired_leaf_is_rejected() {
    let leaf = pki("leaf_expired");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf, &int0], "expired.otter-test.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::CertificateExpired)
    );
}

#[test]
fn not_yet_valid_leaf_is_rejected() {
    let leaf = pki("leaf_exact");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf, &int0], "leaf.otter-test.example", BEFORE_ALL_VALID, &roots).err(),
        Some(X509Error::CertificateNotYetValid)
    );
}

#[test]
fn missing_intermediate_has_no_path() {
    let leaf = pki("leaf_exact");
    let roots = root_store();
    // The leaf's issuer (the intermediate) is not in the pool and is not
    // itself a trust anchor, so no path exists.
    assert_eq!(
        verify_server_chain(&[&leaf], "leaf.otter-test.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::NoPathFound)
    );
}

#[test]
fn self_signed_leaf_is_untrusted() {
    let leaf = pki("self_signed_leaf");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf], "selfsigned.otter-test.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::NoPathFound)
    );
}

#[test]
fn leaf_signed_by_a_non_ca_is_rejected() {
    let leaf = pki("leaf_via_non_ca");
    let non_ca = pki("non_ca");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf, &non_ca], "leaf-via-non-ca.otter-test.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::NoPathFound)
    );
}

#[test]
fn path_len_constraint_is_enforced() {
    let leaf = pki("leaf_under_sub_intermediate");
    let sub = pki("sub_intermediate");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    // intermediate_pathlen0 has pathLen:0 -- sub_intermediate, one more CA
    // below it, is exactly what that forbids.
    assert_eq!(
        verify_server_chain(&[&leaf, &sub, &int0], "pathlenviolation.otter-test.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::NoPathFound)
    );
}

#[test]
fn extended_key_usage_mismatch_is_rejected() {
    let leaf = pki("leaf_wrong_eku");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf, &int0], "wrongeku.otter-test.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::ExtendedKeyUsageMismatch)
    );
}

#[test]
fn name_constraint_violation_is_rejected() {
    let leaf = pki("leaf_nc_violation");
    let int_nc = pki("intermediate_name_constrained");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf, &int_nc], "host.other-domain.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::NoPathFound)
    );
}

#[test]
fn unknown_critical_extension_is_rejected() {
    let leaf = pki("leaf_unknown_critical_ext");
    let int0 = pki("intermediate_pathlen0");
    let roots = root_store();
    assert_eq!(
        verify_server_chain(&[&leaf, &int0], "unknownext.otter-test.example", NOW_NORMAL, &roots).err(),
        Some(X509Error::UnknownCriticalExtension)
    );
}

#[test]
fn empty_chain_and_garbage_are_rejected_not_panicked() {
    let roots = root_store();
    assert!(verify_server_chain(&[], "example.com", NOW_NORMAL, &roots).is_err());
    let garbage = [0u8; 4];
    assert!(verify_server_chain(&[&garbage], "example.com", NOW_NORMAL, &roots).is_err());
}
