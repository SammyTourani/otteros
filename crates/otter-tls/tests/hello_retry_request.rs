//! `HelloRetryRequest` (brief M8-T5): cookie echo and transcript
//! `message_hash` substitution, and the "requested group other than x25519
//! is a clear error" rule.
//!
//! This client's `supported_groups`/`key_share` only ever offer x25519
//! (brief M8-T5's own scope), so no spec-compliant server can ever send a
//! real `HelloRetryRequest` naming a *different* group -- it could only ever
//! re-request x25519 itself (meaningful only for a stateless-cookie retry,
//! which `openssl s_server` has no option to force) or reject the
//! connection outright. Real interop cannot exercise this path either way
//! (documented, per the brief's own fallback for exactly this situation);
//! both cases are instead exercised with hand-built `HelloRetryRequest`
//! bytes fed directly to the sans-I/O state machine -- no socket needed at
//! all, since `read_tls`/`process`/`take_outgoing` never touch one.

mod common;

use common::OsRng;
use otter_tls::{ClientConfig, ClientConnection, TlsError};

const HRR_RANDOM: [u8; 32] = [
    0xcf, 0x21, 0xad, 0x74, 0xe5, 0x9a, 0x61, 0x11, 0xbe, 0x1d, 0x8c, 0x02, 0x1e, 0x65, 0xb8, 0x91, 0xc2, 0xa2, 0x11, 0x16, 0x7a, 0xbb, 0x8c, 0x5e,
    0x07, 0x9e, 0x09, 0xe2, 0xc8, 0xa8, 0x33, 0x9c,
];

fn extension(out: &mut Vec<u8>, ty: u16, body: &[u8]) {
    out.extend_from_slice(&ty.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
}

/// Builds a `HelloRetryRequest` record: legacy_version {3,3}, the RFC 8446
/// section 4.1.4 sentinel `random`, the client's own echoed session id and
/// offered suite, `supported_versions`, an optional `key_share` (selected
/// group only, no key), and an optional `cookie`.
fn build_hrr(session_id: &[u8], cipher_suite: u16, key_share_group: Option<u16>, cookie: Option<&[u8]>) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&0x0303u16.to_be_bytes());
    body.extend_from_slice(&HRR_RANDOM);
    body.push(session_id.len() as u8);
    body.extend_from_slice(session_id);
    body.extend_from_slice(&cipher_suite.to_be_bytes());
    body.push(0); // legacy_compression_method

    let mut ext = Vec::new();
    extension(&mut ext, 43, &0x0304u16.to_be_bytes()); // supported_versions
    if let Some(group) = key_share_group {
        extension(&mut ext, 51, &group.to_be_bytes()); // key_share (HRR form: just the group)
    }
    if let Some(cookie) = cookie {
        let mut cookie_ext = Vec::new();
        cookie_ext.extend_from_slice(&(cookie.len() as u16).to_be_bytes());
        cookie_ext.extend_from_slice(cookie);
        extension(&mut ext, 44, &cookie_ext);
    }
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend_from_slice(&ext);

    let mut message = Vec::new();
    message.push(2); // ServerHello / HelloRetryRequest
    let len = (body.len() as u32).to_be_bytes();
    message.extend_from_slice(&len[1..]);
    message.extend_from_slice(&body);

    let mut record = Vec::new();
    record.extend_from_slice(&[0x16, 0x03, 0x03]);
    record.extend_from_slice(&(message.len() as u16).to_be_bytes());
    record.extend_from_slice(&message);
    record
}

/// Extracts `(legacy_session_id, key_share_public_key)` from a plaintext
/// `ClientHello` record this client just produced.
fn client_hello_session_id_and_key_share(record: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let body = &record[9..]; // skip 5-byte record header + 4-byte handshake header
    let mut pos = 2 + 32; // legacy_version + random
    let session_id_len = body[pos] as usize;
    pos += 1;
    let session_id = body[pos..pos + session_id_len].to_vec();
    pos += session_id_len;
    let suites_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2 + suites_len;
    pos += 1 + body[pos] as usize; // legacy_compression_methods
    let ext_total_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;
    let ext_end = pos + ext_total_len;
    while pos < ext_end {
        let ty = u16::from_be_bytes([body[pos], body[pos + 1]]);
        let len = u16::from_be_bytes([body[pos + 2], body[pos + 3]]) as usize;
        let data = &body[pos + 4..pos + 4 + len];
        if ty == 51 {
            // key_share: list-length(2) + group(2) + key-length(2) + key.
            let key = data[6..].to_vec();
            return (session_id, key);
        }
        pos += 4 + len;
    }
    panic!("ClientHello had no key_share extension");
}

fn find_extension(record: &[u8], want_type: u16) -> Option<Vec<u8>> {
    let body = &record[9..];
    let mut pos = 2 + 32;
    let session_id_len = body[pos] as usize;
    pos += 1 + session_id_len;
    let suites_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2 + suites_len;
    pos += 1 + body[pos] as usize;
    let ext_total_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;
    let ext_end = pos + ext_total_len;
    while pos < ext_end {
        let ty = u16::from_be_bytes([body[pos], body[pos + 1]]);
        let len = u16::from_be_bytes([body[pos + 2], body[pos + 3]]) as usize;
        let data = body[pos + 4..pos + 4 + len].to_vec();
        if ty == want_type {
            return Some(data);
        }
        pos += 4 + len;
    }
    None
}

#[test]
fn cookie_is_echoed_and_key_share_is_reused() {
    let config = ClientConfig::new(otter_x509::TrustStore::empty());
    let mut conn = ClientConnection::new(config, "leaf.otter-test.example", 1_748_736_000, &mut OsRng::new()).unwrap();
    let ch1 = conn.take_outgoing();
    let (session_id, key_share_1) = client_hello_session_id_and_key_share(&ch1);

    let hrr = build_hrr(&session_id, 0x1301, None, Some(b"a-server-issued-cookie"));
    conn.read_tls(&hrr);
    assert!(matches!(conn.process(), Ok(None)), "a HelloRetryRequest produces no caller-visible event by itself");

    let ch2 = conn.take_outgoing();
    assert!(!ch2.is_empty(), "a second ClientHello (plus its compatibility change_cipher_spec) must be queued");
    assert_eq!(ch2[0], 0x16, "the second ClientHello must be sent as a plaintext handshake record");

    let cookie_ext = find_extension(&ch2, 44).expect("second ClientHello must echo the cookie extension");
    // Cookie extension_data is itself length-prefixed (RFC 8446 4.2.2).
    assert_eq!(&cookie_ext[2..], b"a-server-issued-cookie");

    let (_, key_share_2) = client_hello_session_id_and_key_share(&ch2);
    assert_eq!(key_share_1, key_share_2, "the x25519 key share must be reused, not regenerated, when the group does not change");

    assert!(!conn.is_closed());
}

#[test]
fn hello_retry_request_naming_a_non_x25519_group_is_a_clear_error() {
    let config = ClientConfig::new(otter_x509::TrustStore::empty());
    let mut conn = ClientConnection::new(config, "leaf.otter-test.example", 1_748_736_000, &mut OsRng::new()).unwrap();
    let ch1 = conn.take_outgoing();
    let (session_id, _) = client_hello_session_id_and_key_share(&ch1);

    // NamedGroup.secp256r1 = 0x0017: a real group, just never one this
    // client offered a key share (or even a supported_groups entry) for.
    let hrr = build_hrr(&session_id, 0x1301, Some(0x0017), None);
    conn.read_tls(&hrr);
    let err = conn.process().expect_err("a HelloRetryRequest requesting an unsupported group must be rejected");
    assert!(matches!(err, TlsError::IllegalParameter(_)), "got {err:?}");
    assert!(conn.is_closed());
}

#[test]
fn a_second_hello_retry_request_is_rejected() {
    let config = ClientConfig::new(otter_x509::TrustStore::empty());
    let mut conn = ClientConnection::new(config, "leaf.otter-test.example", 1_748_736_000, &mut OsRng::new()).unwrap();
    let ch1 = conn.take_outgoing();
    let (session_id, _) = client_hello_session_id_and_key_share(&ch1);

    let hrr = build_hrr(&session_id, 0x1301, None, Some(b"cookie-one"));
    conn.read_tls(&hrr);
    assert!(matches!(conn.process(), Ok(None)));
    let _ = conn.take_outgoing();

    let second_hrr = build_hrr(&session_id, 0x1301, None, Some(b"cookie-two"));
    conn.read_tls(&second_hrr);
    let err = conn.process().expect_err("a second HelloRetryRequest is never valid");
    assert!(matches!(err, TlsError::UnexpectedMessage(_)), "got {err:?}");
}
