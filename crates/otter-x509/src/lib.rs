//! `otter-x509`: OtterOS's own X.509, written from scratch.
//!
//! DECISIONS.md D22/D27: no crypto or ASN.1/X.509 crates of any kind --
//! strict DER parsing, certificate parsing, RFC 5280 path validation and
//! RFC 6125 host-name matching are all implemented directly against their
//! specifications here, calling into [`otter_crypto`] only for hashing and
//! signature *verification* (this crate never holds or uses a private key).
//! `#![no_std]` + `alloc`, the one path dependency is `otter-crypto`, unit-
//! and integration-tested on the host with `cargo test` (DECISIONS.md D27).
//!
//! ## Layout
//!
//! | Module | What it does |
//! | --- | --- |
//! | [`der`] | Strict, minimal DER reader: definite lengths only, minimal INTEGER/length/OID encoding, clean BIT STRING padding, bounded nesting depth |
//! | [`oid`] | Object identifiers this crate recognizes, as raw DER content bytes |
//! | [`time`] | UTCTime/GeneralizedTime (RFC 5280 4.1.2.5) to Unix seconds |
//! | [`name`] | `Name` (RDN sequence): raw bytes for comparison, parsed attributes for display |
//! | [`spki`] | `SubjectPublicKeyInfo` -> an `otter-crypto` RSA/P-256/P-384 public key |
//! | [`extensions`] | X.509v3 extensions: basicConstraints, keyUsage, extKeyUsage, subjectAltName, AKI/SKI, nameConstraints |
//! | [`sigalg`] | `AlgorithmIdentifier` -> a signature scheme + hash, including RSASSA-PSS parameters |
//! | [`cert`] | `Certificate`: parses a whole DER certificate into the above |
//! | [`hostname`] | RFC 6125 host-name matching against `subjectAltName` |
//! | [`path`] | Path building (with backtracking) and RFC 5280 chain validation; the public [`verify_server_chain`] entry point |
//! | [`roots`] | The embedded Mozilla-derived trust anchors (`scripts/fetch-roots.py`-generated) |
//! | [`error`] | The one [`X509Error`] every fallible operation in this crate returns |
//!
//! Every value this crate ever touches when verifying a server's chain is
//! attacker-supplied except the embedded root store and the caller's own
//! host name and clock, so nothing here is constant-time (as with
//! `otter-crypto`'s own RSA/ECDSA verification, see its `bigint` module
//! doc): the point is to reject malformed or untrustworthy input outright
//! and never panic while doing it, not to hide timing.
#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod cert;
pub mod der;
pub mod error;
pub mod extensions;
pub mod hostname;
pub mod name;
pub mod oid;
pub mod path;
pub mod roots;
pub mod sigalg;
pub mod spki;
pub mod time;

pub use error::X509Error;
pub use path::{PublicKey, TrustStore, VerifiedChain, verify_server_chain};
