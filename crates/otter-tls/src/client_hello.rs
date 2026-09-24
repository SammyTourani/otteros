//! Builds a `ClientHello` (RFC 8446 section 4.1.2), used both for the first
//! one and -- with the same random and key share, plus a cookie if the
//! server asked for one -- the second one sent after a `HelloRetryRequest`.

use alloc::vec::Vec;

use crate::codec::{handshake_type, write_len16_prefixed, write_opaque8, write_u16, write_u8};
use crate::config::ClientConfig;
use crate::extensions;
use crate::suite::CipherSuite;

/// `legacy_version` (RFC 8446 section 4.1.2): fixed at `{3, 3}`; the real
/// negotiated version travels in the `supported_versions` extension.
const LEGACY_VERSION: u16 = 0x0303;

#[allow(clippy::too_many_arguments)]
pub(crate) fn build(
    client_random: [u8; 32],
    legacy_session_id: [u8; 32],
    config: &ClientConfig,
    sni: Option<&[u8]>,
    x25519_public: &[u8; 32],
    cookie: Option<&[u8]>,
) -> Vec<u8> {
    let mut body = Vec::new();
    write_u16(&mut body, LEGACY_VERSION);
    body.extend_from_slice(&client_random);
    // RFC 8446 section 4.1.2 / Appendix D.4: a non-empty, random legacy
    // session id purely for "middlebox compatibility" (makes this look like
    // a resumable TLS 1.2 handshake to anything that only understands that
    // shape); this client never resumes it.
    write_opaque8(&mut body, &legacy_session_id);
    write_len16_prefixed(&mut body, |suites| {
        for suite in CipherSuite::offered(config.prefer_chacha) {
            write_u16(suites, suite.id());
        }
    });
    write_opaque8(&mut body, &[0x00]); // legacy_compression_methods = [null]

    write_len16_prefixed(&mut body, |ext| {
        if let Some(name) = sni {
            extensions::write_server_name(ext, name);
        }
        extensions::write_supported_versions_client(ext);
        extensions::write_supported_groups(ext);
        extensions::write_key_share_client(ext, x25519_public);
        extensions::write_signature_algorithms(ext);
        extensions::write_signature_algorithms_cert(ext);
        extensions::write_alpn(ext, &config.alpn_protocols);
        if let Some(cookie) = cookie {
            extensions::write_cookie(ext, cookie);
        }
    });

    let mut message = Vec::new();
    write_u8(&mut message, handshake_type::CLIENT_HELLO);
    crate::codec::write_u24(&mut message, body.len());
    message.extend_from_slice(&body);
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::parse_handshake_message;

    #[test]
    fn builds_a_well_formed_handshake_message() {
        let config = ClientConfig::new(otter_x509::TrustStore::empty());
        let msg = build(
            [0x11; 32],
            [0x22; 32],
            &config,
            Some(b"example.com"),
            &[0x33; 32],
            None,
        );
        let (parsed, consumed) = parse_handshake_message(&msg).expect("a complete message");
        assert_eq!(consumed, msg.len());
        assert_eq!(parsed.msg_type, handshake_type::CLIENT_HELLO);
        assert_eq!(&parsed.body[0..2], &LEGACY_VERSION.to_be_bytes());
        assert_eq!(&parsed.body[2..34], &[0x11u8; 32]);
    }
}
