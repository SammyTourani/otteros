//! Parses a `ServerHello` body (RFC 8446 section 4.1.3) -- also used for a
//! `HelloRetryRequest`, which is wire-identical apart from its `random`
//! field being a fixed sentinel value (section 4.1.4).

use alloc::vec::Vec;

use crate::codec::Reader;
use crate::error::TlsError;
use crate::extensions;

/// RFC 8446 section 4.1.3: `ServerHello.random` is set to this exact value
/// when the message is really a `HelloRetryRequest`.
const HELLO_RETRY_REQUEST_RANDOM: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91, 0xc2, 0xa2, 0x11,
    0x16, 0x7a, 0xbb, 0x8c, 0x5e, 0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

/// RFC 8446 section 4.1.3: the last 8 bytes of `random` a server that
/// negotiated TLS 1.2 sets, so a TLS 1.3-aware client can detect a
/// downgrade even if a middlebox rewrote everything else.
const DOWNGRADE_TO_TLS_1_2: [u8; 8] = [0x44, 0x4f, 0x57, 0x4e, 0x47, 0x52, 0x44, 0x01];
/// As above, for a downgrade to TLS 1.1 or below.
const DOWNGRADE_TO_TLS_1_1_OR_BELOW: [u8; 8] = [0x44, 0x4f, 0x57, 0x4e, 0x47, 0x52, 0x44, 0x00];

pub(crate) struct ServerHello<'a> {
    pub(crate) is_hello_retry_request: bool,
    pub(crate) random: [u8; 32],
    pub(crate) legacy_session_id_echo: &'a [u8],
    pub(crate) cipher_suite: u16,
    pub(crate) extensions: Vec<(u16, &'a [u8])>,
}

pub(crate) fn parse(body: &[u8]) -> Result<ServerHello<'_>, TlsError> {
    let mut r = Reader::new(body);
    let legacy_version = r.u16()?;
    if legacy_version != 0x0303 {
        return Err(TlsError::Decode("ServerHello.legacy_version is not {3, 3}".into()));
    }
    let random: [u8; 32] = r.take(32)?.try_into().expect("Reader::take(32) returns exactly 32 bytes");
    let legacy_session_id_echo = r.opaque8()?;
    let cipher_suite = r.u16()?;
    let legacy_compression_method = r.u8()?;
    if legacy_compression_method != 0 {
        return Err(TlsError::Decode("ServerHello.legacy_compression_method is not null".into()));
    }
    let ext_bytes = r.opaque16()?;
    if !r.is_empty() {
        return Err(TlsError::Decode("ServerHello has trailing bytes".into()));
    }
    let extensions = extensions::parse_extensions(ext_bytes)?;
    Ok(ServerHello { is_hello_retry_request: random == HELLO_RETRY_REQUEST_RANDOM, random, legacy_session_id_echo, cipher_suite, extensions })
}

/// Whether `random`'s last 8 bytes are one of RFC 8446 section 4.1.3's
/// downgrade sentinels.
pub(crate) fn is_downgrade_sentinel(random: &[u8; 32]) -> bool {
    let tail = &random[24..32];
    tail == DOWNGRADE_TO_TLS_1_2 || tail == DOWNGRADE_TO_TLS_1_1_OR_BELOW
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{write_len16_prefixed, write_opaque8, write_u16, write_u8};

    fn build_basic(random: [u8; 32], session_id: &[u8], cipher_suite: u16) -> Vec<u8> {
        let mut body = Vec::new();
        write_u16(&mut body, 0x0303);
        body.extend_from_slice(&random);
        write_opaque8(&mut body, session_id);
        write_u16(&mut body, cipher_suite);
        write_u8(&mut body, 0);
        write_len16_prefixed(&mut body, |ext| {
            crate::extensions::write_supported_versions_client(ext); // reuse as a stand-in TLV to exercise parsing
        });
        body
    }

    #[test]
    fn parses_a_well_formed_server_hello() {
        let body = build_basic([0x01; 32], &[0xaa, 0xbb], 0x1301);
        let sh = parse(&body).unwrap();
        assert!(!sh.is_hello_retry_request);
        assert_eq!(sh.legacy_session_id_echo, &[0xaa, 0xbb]);
        assert_eq!(sh.cipher_suite, 0x1301);
    }

    #[test]
    fn recognizes_the_hello_retry_request_sentinel() {
        let body = build_basic(HELLO_RETRY_REQUEST_RANDOM, &[], 0x1301);
        let sh = parse(&body).unwrap();
        assert!(sh.is_hello_retry_request);
    }

    #[test]
    fn detects_downgrade_sentinels() {
        let mut r = [0x02u8; 32];
        r[24..32].copy_from_slice(&DOWNGRADE_TO_TLS_1_2);
        assert!(is_downgrade_sentinel(&r));
        let mut r2 = [0x02u8; 32];
        r2[24..32].copy_from_slice(&DOWNGRADE_TO_TLS_1_1_OR_BELOW);
        assert!(is_downgrade_sentinel(&r2));
        assert!(!is_downgrade_sentinel(&[0x02u8; 32]));
    }

    #[test]
    fn rejects_wrong_legacy_version() {
        let mut body = Vec::new();
        write_u16(&mut body, 0x0301);
        body.extend_from_slice(&[0u8; 32]);
        write_opaque8(&mut body, &[]);
        write_u16(&mut body, 0x1301);
        write_u8(&mut body, 0);
        write_len16_prefixed(&mut body, |_| {});
        assert!(parse(&body).is_err());
    }
}
