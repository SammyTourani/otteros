//! `otter-crypto`: OtterOS's own cryptography, written from scratch.
//!
//! DECISIONS.md D22: no crypto crates of any kind -- every primitive here is
//! implemented directly against its specification (FIPS 180-4, RFC 2104/4231,
//! RFC 5869, RFC 8439) and proven against those RFCs' own test vectors plus
//! Project Wycheproof. Secret-dependent code avoids secret-indexed table lookups
//! and secret-dependent branches; see [`ct`] for the shared constant-time
//! helpers. DECISIONS.md D27: `#![no_std]` + `alloc`, zero external
//! dependencies, unit-tested on the host with `cargo test`.
//!
//! `std` is enabled only under `cfg(test)` so integration tests can read the
//! generated vector fixtures from disk; it adds no Cargo dependency and the
//! crate is never built with `std` outside test builds.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod aead;
pub mod chacha20;
pub mod ct;
pub mod hash;
pub mod hkdf;
pub mod hmac;
pub mod poly1305;
pub mod sha256;
pub mod sha512;

pub use aead::{AeadError, open_in_place, seal_in_place};
pub use hash::{Digest, Hash};
pub use hkdf::{HkdfError, hkdf_expand, hkdf_extract};
pub use hmac::Hmac;
pub use sha256::Sha256;
pub use sha512::{Sha384, Sha512};
