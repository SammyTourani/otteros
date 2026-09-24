//! Shared helpers for otter-x509's integration tests: loading DER fixtures
//! written by `scripts/make-test-pki.sh` and `scripts/capture-chains.py`.
//! Lives at `tests/common/mod.rs` (a subdirectory, not `tests/common.rs`) so
//! cargo does not treat this file as its own top-level integration-test
//! binary -- the same pattern `otter-crypto`'s `tests/common/mod.rs` uses.
#![allow(dead_code)] // not every test file uses every helper.

use std::path::PathBuf;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Reads one `scripts/make-test-pki.sh`-generated DER certificate by name
/// (without the `.der` extension), e.g. `pki("leaf_exact")`.
pub fn pki(name: &str) -> Vec<u8> {
    let path = fixtures_dir().join("pki").join(format!("{name}.der"));
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("could not read {path:?} ({e}); run scripts/make-test-pki.sh first"))
}

/// A reference Unix time within every "normally valid" test-PKI
/// certificate's validity window (2024-01-01..2034-01-01Z): 2025-06-01T00:00:00Z.
pub const NOW_NORMAL: u64 = 1_748_736_000;

/// A Unix time after every test-PKI certificate's 2034-01-01Z `notAfter`:
/// 2035-01-02T00:00:00Z.
pub const AFTER_ALL_EXPIRED: u64 = 2_051_308_800;

/// A Unix time before every test-PKI certificate's 2024-01-01Z `notBefore`:
/// 2023-01-01T00:00:00Z.
pub const BEFORE_ALL_VALID: u64 = 1_672_531_200;
