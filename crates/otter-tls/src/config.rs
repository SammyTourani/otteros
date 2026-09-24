//! [`ClientConfig`]: the caller-supplied policy for a connection (trust
//! anchors, ALPN protocols to offer, and which cipher-suite family to
//! prefer).

use alloc::vec::Vec;

/// Configuration shared across (potentially many) [`crate::ClientConnection`]s.
pub struct ClientConfig {
    /// Trust anchors `otter_x509::verify_server_chain` validates a server's
    /// certificate chain against.
    pub roots: otter_x509::TrustStore,
    /// ALPN protocol names to offer, most preferred first (RFC 7301). Empty
    /// means "do not send an ALPN extension".
    pub alpn_protocols: Vec<Vec<u8>>,
    /// Whether `ClientHello.cipher_suites` lists
    /// `TLS_CHACHA20_POLY1305_SHA256` first (brief M8-T5: true when the CPU
    /// lacks AES-NI, since AES-GCM without hardware support is far slower
    /// than ChaCha20-Poly1305's constant-time software path).
    pub prefer_chacha: bool,
}

impl ClientConfig {
    /// Builds a config trusting `roots`, no ALPN, and `prefer_chacha`
    /// defaulted from this host's own CPU (`otter_crypto::hw::aes_ni_available`).
    pub fn new(roots: otter_x509::TrustStore) -> ClientConfig {
        ClientConfig { roots, alpn_protocols: Vec::new(), prefer_chacha: !otter_crypto::hw::aes_ni_available() }
    }

    /// Sets the ALPN protocols to offer.
    pub fn with_alpn_protocols(mut self, protocols: Vec<Vec<u8>>) -> ClientConfig {
        self.alpn_protocols = protocols;
        self
    }

    /// Overrides the cipher-suite preference order.
    pub fn with_prefer_chacha(mut self, prefer_chacha: bool) -> ClientConfig {
        self.prefer_chacha = prefer_chacha;
        self
    }
}
