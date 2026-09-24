//! Validates `scripts/capture-chains.py`'s point-in-time captures of six
//! real public hosts' certificate chains against otter-x509's embedded
//! Mozilla-derived root store (brief M8-T4), at the Unix time they were
//! captured -- proof this crate's DER parsing, path building and signature
//! verification work against certificates it did not create itself.

mod common;

use std::fs;
use std::path::PathBuf;

use otter_x509::{X509Error, roots};

fn chains_dir() -> PathBuf {
    common::fixtures_dir().join("chains")
}

/// Reads `manifest.txt` (`host|cert_count|capture_unix_time`, `#`-prefixed
/// comments skipped) written by `scripts/capture-chains.py`.
fn read_manifest() -> Vec<(String, usize, u64)> {
    let path = chains_dir().join("manifest.txt");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {path:?} ({e}); run scripts/capture-chains.py first"));
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let mut parts = line.split('|');
            let host = parts.next().unwrap().to_string();
            let count: usize = parts.next().unwrap().parse().unwrap();
            let capture_time: u64 = parts.next().unwrap().parse().unwrap();
            (host, count, capture_time)
        })
        .collect()
}

fn load_chain(host: &str, count: usize) -> Vec<Vec<u8>> {
    (0..count).map(|i| fs::read(chains_dir().join(format!("{host}_{i}.der"))).unwrap()).collect()
}

#[test]
fn every_captured_host_chain_validates_at_capture_time() {
    let manifest = read_manifest();
    assert_eq!(manifest.len(), 6, "expected all six hosts from the brief to have been captured");

    let store = roots::trust_store();
    assert!(store.len() > 50, "embedded root store looks too small ({} roots)", store.len());

    for (host, count, capture_time) in &manifest {
        let der_owned = load_chain(host, *count);
        let der_refs: Vec<&[u8]> = der_owned.iter().map(Vec::as_slice).collect();
        let result = otter_x509::verify_server_chain(&der_refs, host, *capture_time, &store);
        let chain = result.unwrap_or_else(|e| panic!("{host}: expected chain to verify at capture time, got {e:?}"));
        println!("{host}: leaf subject = {}", chain.leaf_subject());
    }
}

#[test]
fn captured_chains_reject_the_wrong_host_name() {
    let manifest = read_manifest();
    let store = roots::trust_store();
    let (host, count, capture_time) = &manifest[0];
    let der_owned = load_chain(host, *count);
    let der_refs: Vec<&[u8]> = der_owned.iter().map(Vec::as_slice).collect();
    let wrong_host = "definitely-not-the-right-host.invalid";
    assert_eq!(
        otter_x509::verify_server_chain(&der_refs, wrong_host, *capture_time, &store).err(),
        Some(X509Error::HostnameMismatch)
    );
}

#[test]
fn captured_chains_are_rejected_against_an_empty_trust_store() {
    let manifest = read_manifest();
    let empty = otter_x509::TrustStore::empty();
    let (host, count, capture_time) = &manifest[0];
    let der_owned = load_chain(host, *count);
    let der_refs: Vec<&[u8]> = der_owned.iter().map(Vec::as_slice).collect();
    assert_eq!(
        otter_x509::verify_server_chain(&der_refs, host, *capture_time, &empty).err(),
        Some(X509Error::NoPathFound)
    );
}
