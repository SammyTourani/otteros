//! The handshake transcript (RFC 8446 section 4.4.1): every handshake
//! message that counts towards `Transcript-Hash`, concatenated raw. The
//! negotiated hash algorithm is not known until `ServerHello` (or a
//! `HelloRetryRequest`) arrives, so this buffers plain bytes rather than
//! hashing incrementally -- a full handshake's messages are at most tens of
//! kilobytes (a generous certificate chain, mainly), trivial to keep around
//! for a connection's lifetime, and buffering means [`Transcript::hash`] can
//! simply be called again at every checkpoint RFC 8446 needs one
//! (`ServerHello`, `CertificateVerify`, both `Finished`s) without any
//! "which hash did I start this stream with" bookkeeping.

use alloc::vec::Vec;

use crate::codec::{handshake_type, write_u24, write_u8};
use crate::suite::SuiteHash;

pub(crate) struct Transcript {
    raw: Vec<u8>,
}

impl Transcript {
    pub(crate) fn new() -> Transcript {
        Transcript { raw: Vec::new() }
    }

    /// Appends one handshake message's raw bytes (header and body).
    pub(crate) fn add(&mut self, message_bytes: &[u8]) {
        self.raw.extend_from_slice(message_bytes);
    }

    /// RFC 8446 section 4.4.1's `HelloRetryRequest` transcript substitution:
    /// replaces the buffered `ClientHello1` bytes (the only thing in the
    /// transcript so far, at the point this must be called -- right after a
    /// `HelloRetryRequest`'s own `cipher_suite` fixes which hash to use, and
    /// before that `HelloRetryRequest` itself is added) with a synthetic
    /// `message_hash` message: `Hash(ClientHello1)` wrapped in the same
    /// 4-byte handshake header any real message would have.
    pub(crate) fn replace_client_hello1_with_message_hash(&mut self, hash: SuiteHash) {
        let digest = hash.digest(&self.raw);
        let mut synthetic = Vec::with_capacity(4 + digest.as_bytes().len());
        write_u8(&mut synthetic, handshake_type::MESSAGE_HASH);
        write_u24(&mut synthetic, digest.as_bytes().len());
        synthetic.extend_from_slice(digest.as_bytes());
        self.raw = synthetic;
    }

    pub(crate) fn hash(&self, hash: SuiteHash) -> alloc::vec::Vec<u8> {
        hash.digest(&self.raw).as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_retry_request_substitution_matches_rfc_8446_shape() {
        let mut t = Transcript::new();
        t.add(b"pretend client hello 1 bytes");
        t.replace_client_hello1_with_message_hash(SuiteHash::Sha256);
        // 1 byte type + 3 byte length + 32 byte SHA-256 digest.
        assert_eq!(t.raw.len(), 1 + 3 + 32);
        assert_eq!(t.raw[0], handshake_type::MESSAGE_HASH);
        assert_eq!(u32::from_be_bytes([0, t.raw[1], t.raw[2], t.raw[3]]) as usize, 32);
    }

    #[test]
    fn hash_changes_as_messages_are_added() {
        let mut t = Transcript::new();
        let h0 = t.hash(SuiteHash::Sha256);
        t.add(b"a message");
        let h1 = t.hash(SuiteHash::Sha256);
        assert_ne!(h0, h1);
        t.add(b"another message");
        let h2 = t.hash(SuiteHash::Sha256);
        assert_ne!(h1, h2);
    }
}
