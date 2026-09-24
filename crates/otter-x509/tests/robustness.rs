//! "Corrupting fixture DER at every byte offset never panics" (brief
//! M8-T4's robustness requirement): flips every single byte, one at a
//! time, of every certificate fixture this crate has (the test PKI, the
//! six captured real-world chains) and feeds the result through the public
//! [`verify_server_chain`] entry point, asserting the call never panics --
//! only ever returns `Ok` or `Err`. A hostile or merely corrupted
//! certificate is exactly the input this crate's strict DER/X.509 parsing
//! exists to reject cleanly.

mod common;

use std::panic::{self, AssertUnwindSafe};

use otter_x509::{TrustStore, verify_server_chain};

const PKI_FIXTURES: &[&str] = &[
    "root",
    "intermediate_pathlen0",
    "leaf_exact",
    "leaf_wildcard",
    "leaf_ip",
    "leaf_wrong_eku",
    "leaf_unknown_critical_ext",
    "leaf_expired",
    "sub_intermediate",
    "leaf_under_sub_intermediate",
    "non_ca",
    "leaf_via_non_ca",
    "intermediate_name_constrained",
    "leaf_nc_ok",
    "leaf_nc_violation",
    "self_signed_leaf",
];

fn all_fixture_ders() -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = PKI_FIXTURES.iter().map(|name| (format!("pki/{name}"), common::pki(name))).collect();

    let chains_dir = common::fixtures_dir().join("chains");
    let mut entries: Vec<_> = std::fs::read_dir(&chains_dir)
        .unwrap_or_else(|e| panic!("could not read {chains_dir:?} ({e}); run scripts/capture-chains.py first"))
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "der") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let data = std::fs::read(&path).unwrap();
            out.push((format!("chains/{name}"), data));
        }
    }
    out
}

#[test]
fn flipping_any_single_byte_of_any_fixture_never_panics() {
    let fixtures = all_fixture_ders();
    assert!(fixtures.len() >= 30, "expected fixtures from both scripts/make-test-pki.sh and scripts/capture-chains.py");

    // The default panic hook prints every caught panic's message and
    // location to stderr; with potentially thousands of corrupted-input
    // attempts, a single genuine bug would otherwise flood the output.
    // Restored unconditionally (even on an assertion failure) via a guard.
    type PanicHook = dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send;
    struct RestoreHook(Option<Box<PanicHook>>);
    impl Drop for RestoreHook {
        fn drop(&mut self) {
            if let Some(hook) = self.0.take() {
                panic::set_hook(hook);
            }
        }
    }
    let previous_hook = panic::take_hook();
    let _restore = RestoreHook(Some(previous_hook));
    panic::set_hook(Box::new(|_| {}));

    let roots = TrustStore::empty();
    let mut total_offsets = 0usize;
    let mut panicked: Vec<String> = Vec::new();

    for (name, original) in &fixtures {
        for offset in 0..original.len() {
            total_offsets += 1;
            let mut corrupted = original.clone();
            corrupted[offset] ^= 0xff;
            let chain: [&[u8]; 1] = [&corrupted];
            let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                verify_server_chain(&chain, "example.com", 1_800_000_000, &roots)
            }));
            if outcome.is_err() {
                panicked.push(format!("{name} byte {offset}"));
            }
        }
    }

    assert!(total_offsets > 10_000, "expected to have exercised a substantial number of byte offsets, got {total_offsets}");
    assert!(panicked.is_empty(), "corrupting a single byte panicked at: {panicked:?}");
}
