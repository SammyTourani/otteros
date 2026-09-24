//! `otter-tls`: OtterOS's own TLS 1.3 client, written from scratch against
//! RFC 8446.
//!
//! DECISIONS.md D22: TLS 1.3 only (no earlier version is ever negotiated;
//! any verification failure aborts the handshake) and D2/D27: `#![no_std]` +
//! `alloc`, zero external dependencies, the two path dependencies allowed
//! are `otter-crypto` (the record layer's AEADs, hashes, HKDF and X25519)
//! and `otter-x509` (certificate chain validation) -- this crate implements
//! only the TLS protocol state machine itself: the handshake, the key
//! schedule (RFC 8446 section 7.1), and the record layer (RFC 8446 section
//! 5), calling into those two crates for every actual cryptographic
//! operation and never touching key material they do not already handle
//! (this crate does not, for instance, reimplement HKDF or read a
//! certificate's DER bytes itself).
//!
//! ## Design: sans-I/O
//!
//! [`ClientConnection`] owns no socket. The caller (a plain
//! `std::net::TcpStream` in this crate's own tests today; OtterOS's own TCP
//! stack, unchanged, once it exists) drives it entirely through:
//!
//! - [`ClientConnection::read_tls`] -- feed bytes received from the network.
//! - [`ClientConnection::process`] -- advance the state machine, returning
//!   the next [`Event`] or `Ok(None)` when more input is needed.
//! - [`ClientConnection::take_outgoing`] -- drain bytes to send.
//! - [`ClientConnection::send`] -- queue application data to send once
//!   [`Event::HandshakeComplete`] has been observed.
//! - [`ClientConnection::close`] -- send `close_notify` and stop.
//!
//! The caller supplies randomness through the [`Rng`] trait and the current
//! time as Unix seconds (for [`otter_x509::verify_server_chain`]); this
//! crate has no other source of either, which is exactly what makes it
//! `no_std`-portable and unit-testable on the host without a real clock or
//! entropy source of its own.
//!
//! ## Scope (brief M8-T5)
//!
//! `ClientHello` offers `TLS_AES_128_GCM_SHA256`, `TLS_AES_256_GCM_SHA384`
//! and `TLS_CHACHA20_POLY1305_SHA256`; X25519 only for key exchange; ECDSA
//! P-256/P-384 and RSASSA-PSS (SHA-256/384/512) for `CertificateVerify`; SNI,
//! ALPN. No client certificates, no PSK/session resumption, no early data.
//! `HelloRetryRequest`, `KeyUpdate` and `close_notify` are all handled;
//! `NewSessionTicket` is silently discarded.

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

mod alert;
mod cert_verify;
mod certificate;
mod client_hello;
mod codec;
mod config;
mod conn;
mod error;
mod extensions;
mod key_schedule;
mod record;
mod rng;
mod server_hello;
mod signature_scheme;
mod suite;
mod transcript;

pub use alert::{AlertDescription, AlertLevel};
pub use config::ClientConfig;
pub use conn::{ClientConnection, Event};
pub use error::TlsError;
pub use rng::Rng;
pub use suite::CipherSuite;
