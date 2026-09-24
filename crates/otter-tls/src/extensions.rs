//! `Extension` encoding for the messages this client sends (`ClientHello`)
//! and decoding for the ones it receives (`ServerHello`,
//! `HelloRetryRequest`, `EncryptedExtensions`) -- RFC 8446 section 4.2.

use alloc::vec::Vec;

use crate::codec::{Reader, write_len16_prefixed, write_opaque8, write_opaque16, write_u16};
use crate::error::TlsError;
use crate::signature_scheme::{SIGNATURE_ALGORITHMS, SIGNATURE_ALGORITHMS_CERT, encode_list};

pub(crate) mod ext_type {
    pub(crate) const SERVER_NAME: u16 = 0;
    pub(crate) const SUPPORTED_GROUPS: u16 = 10;
    pub(crate) const SIGNATURE_ALGORITHMS: u16 = 13;
    pub(crate) const ALPN: u16 = 16;
    pub(crate) const SUPPORTED_VERSIONS: u16 = 43;
    pub(crate) const COOKIE: u16 = 44;
    pub(crate) const SIGNATURE_ALGORITHMS_CERT: u16 = 50;
    pub(crate) const KEY_SHARE: u16 = 51;
}

/// `NamedGroup.x25519` (RFC 8446 section 4.2.7): the only key-exchange group
/// this client offers.
pub(crate) const X25519_GROUP: u16 = 0x001d;

/// TLS 1.3's version code point (RFC 8446 section 4.2.1).
pub(crate) const TLS_1_3_VERSION: u16 = 0x0304;

fn push_extension(out: &mut Vec<u8>, ext_type: u16, body: impl FnOnce(&mut Vec<u8>)) {
    write_u16(out, ext_type);
    write_len16_prefixed(out, body);
}

// ---- ClientHello extension encoders -------------------------------------

/// `server_name` (RFC 6066 section 3): a `ServerNameList` of one
/// `host_name`-typed entry -- `u16 list_len; u8 name_type=0; opaque
/// host_name<1..2^16-1>`.
pub(crate) fn write_server_name(out: &mut Vec<u8>, dns_name: &[u8]) {
    push_extension(out, ext_type::SERVER_NAME, |data| {
        write_len16_prefixed(data, |list| {
            list.push(0); // NameType::host_name
            write_opaque16(list, dns_name);
        });
    });
}

pub(crate) fn write_supported_versions_client(out: &mut Vec<u8>) {
    push_extension(out, ext_type::SUPPORTED_VERSIONS, |data| {
        write_opaque8(data, &TLS_1_3_VERSION.to_be_bytes());
    });
}

pub(crate) fn write_supported_groups(out: &mut Vec<u8>) {
    push_extension(out, ext_type::SUPPORTED_GROUPS, |data| {
        write_opaque16(data, &X25519_GROUP.to_be_bytes());
    });
}

pub(crate) fn write_key_share_client(out: &mut Vec<u8>, x25519_public: &[u8; 32]) {
    push_extension(out, ext_type::KEY_SHARE, |data| {
        write_len16_prefixed(data, |list| {
            write_u16(list, X25519_GROUP);
            write_opaque16(list, x25519_public);
        });
    });
}

pub(crate) fn write_signature_algorithms(out: &mut Vec<u8>) {
    push_extension(out, ext_type::SIGNATURE_ALGORITHMS, |data| encode_list(data, SIGNATURE_ALGORITHMS));
}

pub(crate) fn write_signature_algorithms_cert(out: &mut Vec<u8>) {
    push_extension(out, ext_type::SIGNATURE_ALGORITHMS_CERT, |data| encode_list(data, SIGNATURE_ALGORITHMS_CERT));
}

pub(crate) fn write_alpn(out: &mut Vec<u8>, protocols: &[Vec<u8>]) {
    if protocols.is_empty() {
        return;
    }
    push_extension(out, ext_type::ALPN, |data| {
        write_len16_prefixed(data, |list| {
            for p in protocols {
                write_opaque8(list, p);
            }
        });
    });
}

pub(crate) fn write_cookie(out: &mut Vec<u8>, cookie: &[u8]) {
    push_extension(out, ext_type::COOKIE, |data| write_opaque16(data, cookie));
}

// ---- Decoding (ServerHello / HelloRetryRequest / EncryptedExtensions) ---

/// Parses a flat `Extension extensions<...>` list's *content* bytes (the
/// outer length prefix already consumed by the caller) into `(type, data)`
/// pairs.
pub(crate) fn parse_extensions(data: &[u8]) -> Result<Vec<(u16, &[u8])>, TlsError> {
    let mut r = Reader::new(data);
    let mut out = Vec::new();
    while !r.is_empty() {
        let ty = r.u16()?;
        let body = r.opaque16()?;
        out.push((ty, body));
    }
    Ok(out)
}

pub(crate) fn find<'a>(exts: &[(u16, &'a [u8])], ty: u16) -> Option<&'a [u8]> {
    exts.iter().find(|(t, _)| *t == ty).map(|(_, b)| *b)
}

/// `supported_versions` in a `ServerHello`/`HelloRetryRequest`: a bare
/// `uint16` (unlike the length-prefixed list a `ClientHello` sends).
pub(crate) fn parse_supported_versions_server(body: &[u8]) -> Result<u16, TlsError> {
    let mut r = Reader::new(body);
    let v = r.u16()?;
    if !r.is_empty() {
        return Err(TlsError::Decode("supported_versions (server) has trailing bytes".into()));
    }
    Ok(v)
}

/// `key_share` in a `ServerHello`: one `KeyShareEntry` (`uint16 group;
/// opaque key_exchange<1..2^16-1>`), not a list.
pub(crate) fn parse_key_share_server_hello(body: &[u8]) -> Result<(u16, &[u8]), TlsError> {
    let mut r = Reader::new(body);
    let group = r.u16()?;
    let key = r.opaque16()?;
    if !r.is_empty() {
        return Err(TlsError::Decode("key_share (ServerHello) has trailing bytes".into()));
    }
    Ok((group, key))
}

/// `key_share` in a `HelloRetryRequest`: just the selected `NamedGroup`.
pub(crate) fn parse_key_share_hrr(body: &[u8]) -> Result<u16, TlsError> {
    let mut r = Reader::new(body);
    let group = r.u16()?;
    if !r.is_empty() {
        return Err(TlsError::Decode("key_share (HelloRetryRequest) has trailing bytes".into()));
    }
    Ok(group)
}

/// `cookie`'s `extension_data` is itself `struct { opaque cookie<1..2^16-1>;
/// }` (RFC 8446 section 4.2.2): one more length prefix to strip.
pub(crate) fn parse_cookie(body: &[u8]) -> Result<&[u8], TlsError> {
    let mut r = Reader::new(body);
    let cookie = r.opaque16()?;
    if !r.is_empty() {
        return Err(TlsError::Decode("cookie extension has trailing bytes".into()));
    }
    Ok(cookie)
}

/// `EncryptedExtensions`' `application_layer_protocol_negotiation`: a
/// `ProtocolNameList` with exactly one entry once negotiated.
pub(crate) fn parse_alpn_response(body: &[u8]) -> Result<Vec<u8>, TlsError> {
    let mut r = Reader::new(body);
    let selected = r.nested16(|list| {
        let name = list.opaque8()?;
        if !list.is_empty() {
            return Err(TlsError::Decode("ALPN response named more than one protocol".into()));
        }
        Ok(name.to_vec())
    })?;
    if !r.is_empty() {
        return Err(TlsError::Decode("ALPN extension has trailing bytes".into()));
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_name_extension_round_trip_shape() {
        let mut out = Vec::new();
        write_server_name(&mut out, b"example.com");
        // type(2)=0 + ext_len(2) + list_len(2) + name_type(1)=0 + host_len(2) + host(11)
        assert_eq!(out[0..2], [0x00, 0x00]);
        let ext_len = u16::from_be_bytes([out[2], out[3]]) as usize;
        assert_eq!(ext_len, out.len() - 4);
        let list_len = u16::from_be_bytes([out[4], out[5]]) as usize;
        assert_eq!(list_len, ext_len - 2);
        assert_eq!(out[6], 0); // NameType::host_name
        let host_len = u16::from_be_bytes([out[7], out[8]]) as usize;
        assert_eq!(host_len, b"example.com".len());
        assert_eq!(&out[9..9 + host_len], b"example.com");
    }

    #[test]
    fn parse_extensions_round_trips_written_extensions() {
        let mut out = Vec::new();
        write_supported_versions_client(&mut out);
        write_supported_groups(&mut out);
        let parsed = parse_extensions(&out).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, ext_type::SUPPORTED_VERSIONS);
        assert_eq!(parsed[0].1, &[0x02, 0x03, 0x04]);
        assert_eq!(parsed[1].0, ext_type::SUPPORTED_GROUPS);
        assert_eq!(parsed[1].1, &[0x00, 0x02, 0x00, 0x1d]);
    }

    #[test]
    fn supported_versions_server_parses_exactly_two_bytes() {
        assert_eq!(parse_supported_versions_server(&[0x03, 0x04]).unwrap(), 0x0304);
        assert!(parse_supported_versions_server(&[0x03, 0x04, 0x00]).is_err());
    }

    #[test]
    fn key_share_server_hello_round_trips() {
        let mut out = Vec::new();
        write_key_share_client(&mut out, &[0x42; 32]);
        let exts = parse_extensions(&out).unwrap();
        // Re-parse as if it were a ServerHello's single KeyShareEntry: strip
        // the ClientHello's outer list-length (2 bytes) that
        // parse_key_share_server_hello does not expect.
        let list = exts[0].1;
        let (group, key) = parse_key_share_server_hello(&list[2..]).unwrap();
        assert_eq!(group, X25519_GROUP);
        assert_eq!(key, &[0x42; 32]);
    }

    #[test]
    fn cookie_round_trips() {
        let mut out = Vec::new();
        write_cookie(&mut out, b"a cookie value");
        let exts = parse_extensions(&out).unwrap();
        assert_eq!(parse_cookie(exts[0].1).unwrap(), b"a cookie value");
    }

    #[test]
    fn alpn_response_parses_single_protocol() {
        let mut out = Vec::new();
        write_alpn(&mut out, &[b"h2".to_vec()]);
        let exts = parse_extensions(&out).unwrap();
        assert_eq!(parse_alpn_response(exts[0].1).unwrap(), b"h2");
    }
}
