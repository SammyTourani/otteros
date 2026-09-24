//! `otter-crypto`: OtterOS's own cryptography, written from scratch.
//!
//! DECISIONS.md D22: no crypto crates of any kind -- every primitive here is
//! implemented directly against its specification (FIPS 180-4, FIPS 197,
//! RFC 2104/4231, RFC 5869, RFC 7748, RFC 8439, NIST SP 800-38D) and proven
//! against those specs' own test vectors plus Project Wycheproof.
//! Secret-dependent code avoids secret-indexed table lookups and
//! secret-dependent branches; see [`ct`] for the shared constant-time helpers.
//! AES uses this constant-time software path unless AES-NI is present, in
//! which case [`hw`] (CPUID-gated, the crate's only `unsafe`) is used instead;
//! either way GCM's own logic (padding, counters, tag comparison) stays the
//! same. DECISIONS.md D27: `#![no_std]` + `alloc`, zero external dependencies,
//! unit-tested on the host with `cargo test`.
//!
//! `std` is enabled only under `cfg(test)` so integration tests can read the
//! generated vector fixtures from disk; it adds no Cargo dependency and the
//! crate is never built with `std` outside test builds.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod aead;
pub mod aes;
pub mod bigint;
pub mod chacha20;
pub mod ct;
pub mod ecc;
pub mod gcm;
pub mod ghash;
pub mod hash;
pub mod hkdf;
pub mod hmac;
pub mod hw;
pub mod p256;
pub mod p384;
pub mod poly1305;
pub mod rsa;
pub mod sha256;
pub mod sha512;
pub mod x25519;

pub use aead::{Aead, AeadError, open_in_place, seal_in_place};
pub use gcm::{Aes128Gcm, Aes256Gcm};
pub use hash::{Digest, Hash};
pub use hkdf::{HkdfError, hkdf_expand, hkdf_extract};
pub use hmac::Hmac;
pub use sha256::Sha256;
pub use sha512::{Sha384, Sha512};
pub use x25519::{X25519Error, x25519, x25519_base};
