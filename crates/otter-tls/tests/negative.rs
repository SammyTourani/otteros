//! Negative tests (brief M8-T5): every way a handshake must fail cleanly --
//! a certificate that otter_x509 rejects for one reason or another (wrong
//! host, untrusted root, expired leaf), a record corrupted in transit
//! (`bad_record_mac`), a connection cut off mid-handshake, and a server
//! that selects a cipher suite this client never offered.

mod common;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use common::{OsRng, connect, pump_until_event, server_fixtures_dir, start_s_server, test_trust_store};
use otter_tls::{ClientConfig, ClientConnection, TlsError};

fn new_conn(config: ClientConfig, host: &str, now_unix: u64) -> ClientConnection {
    ClientConnection::new(config, host, now_unix, &mut OsRng::new()).expect("ClientConnection::new")
}

#[test]
fn wrong_host_name_is_rejected() {
    let port = common::free_port();
    let fixtures = server_fixtures_dir();
    let _server = start_s_server(port, &fixtures.join("leaf.pem"), &fixtures.join("leaf.key"), &fixtures.join("intermediate.pem"), None);
    let mut stream = connect(port);

    // The server's leaf is for "leaf.otter-test.example"; ask for a
    // completely different (but syntactically valid) name instead.
    let config = ClientConfig::new(test_trust_store());
    let mut conn = new_conn(config, "not-the-right-host.example", common::NOW_NORMAL);
    let err = pump_until_event(&mut conn, &mut stream).expect_err("must reject a hostname mismatch");
    assert!(matches!(err, TlsError::Certificate(otter_x509::X509Error::HostnameMismatch)), "got {err:?}");
}

#[test]
fn untrusted_root_is_rejected() {
    let port = common::free_port();
    let fixtures = server_fixtures_dir();
    let _server = start_s_server(port, &fixtures.join("leaf.pem"), &fixtures.join("leaf.key"), &fixtures.join("intermediate.pem"), None);
    let mut stream = connect(port);

    // The real, embedded Mozilla-derived root store has never heard of this
    // from-scratch test PKI's root.
    let config = ClientConfig::new(otter_x509::roots::trust_store());
    let mut conn = new_conn(config, "leaf.otter-test.example", common::NOW_NORMAL);
    let err = pump_until_event(&mut conn, &mut stream).expect_err("must reject an untrusted root");
    assert!(matches!(err, TlsError::Certificate(otter_x509::X509Error::NoPathFound)), "got {err:?}");
}

#[test]
fn expired_leaf_is_rejected_when_validated_at_a_later_time() {
    let port = common::free_port();
    let fixtures = server_fixtures_dir();
    let _server = start_s_server(port, &fixtures.join("expired_leaf.pem"), &fixtures.join("expired_leaf.key"), &fixtures.join("intermediate.pem"), None);
    let mut stream = connect(port);

    // leaf_expired's validity window is 2020-01-01..2020-02-01Z; validate
    // long after that.
    let config = ClientConfig::new(test_trust_store());
    let mut conn = new_conn(config, "expired.otter-test.example", common::AFTER_ALL_EXPIRED);
    let err = pump_until_event(&mut conn, &mut stream).expect_err("must reject an expired leaf");
    assert!(matches!(err, TlsError::Certificate(otter_x509::X509Error::CertificateExpired)), "got {err:?}");
}

/// A TCP proxy sitting between the client and a real `openssl s_server`,
/// forwarding bytes unchanged in both directions except that it flips one
/// bit of the *third* server-to-client TLS record it forwards: record 0 is
/// the plaintext `ServerHello`, record 1 is `openssl s_server`'s own
/// middlebox-compatibility `change_cipher_spec` (also plaintext, and
/// corrupting *that* would just be rejected as a malformed
/// `change_cipher_spec`, not what this test wants), and record 2 is the
/// server's first genuinely AEAD-protected record (`EncryptedExtensions`,
/// possibly coalesced with whatever follows it) -- flipping a bit there is
/// always deep inside ciphertext or the tag, so it can only ever surface as
/// a MAC failure.
fn spawn_corrupting_proxy(upstream_port: u16) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy listener");
    let proxy_port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let Ok((client_conn, _)) = listener.accept() else { return };
        let upstream = TcpStream::connect(("127.0.0.1", upstream_port)).expect("connect to upstream s_server");
        let client_to_upstream = client_conn.try_clone().unwrap();
        let mut upstream_write = upstream.try_clone().unwrap();
        let t1 = thread::spawn(move || {
            let mut c = client_to_upstream;
            let _ = std::io::copy(&mut c, &mut upstream_write);
        });
        forward_and_corrupt(upstream, client_conn);
        let _ = t1.join();
    });
    proxy_port
}

/// Reads whole TLS records from `upstream` and forwards them to
/// `downstream`, flipping the last ciphertext byte of the second record.
fn forward_and_corrupt(mut upstream: TcpStream, mut downstream: TcpStream) {
    let mut record_index = 0usize;
    loop {
        let mut header = [0u8; 5];
        if upstream.read_exact(&mut header).is_err() {
            return;
        }
        let len = u16::from_be_bytes([header[3], header[4]]) as usize;
        let mut body = vec![0u8; len];
        if upstream.read_exact(&mut body).is_err() {
            return;
        }
        if record_index == 2 {
            let last = body.len() - 1;
            body[last] ^= 0x01;
        }
        record_index += 1;
        if downstream.write_all(&header).is_err() || downstream.write_all(&body).is_err() {
            return;
        }
    }
}

#[test]
fn corrupted_record_is_rejected_as_bad_record_mac() {
    let upstream_port = common::free_port();
    let fixtures = server_fixtures_dir();
    let _server = start_s_server(upstream_port, &fixtures.join("leaf.pem"), &fixtures.join("leaf.key"), &fixtures.join("intermediate.pem"), None);
    // Make sure the upstream is actually accepting before the proxy dials it.
    let _ = connect(upstream_port);
    let proxy_port = spawn_corrupting_proxy(upstream_port);
    let mut stream = connect(proxy_port);

    let config = ClientConfig::new(test_trust_store());
    let mut conn = new_conn(config, "leaf.otter-test.example", common::NOW_NORMAL);
    let err = pump_until_event(&mut conn, &mut stream).expect_err("a corrupted record must not be accepted");
    assert_eq!(err, TlsError::BadRecordMac, "got {err:?}");
}

#[test]
fn truncated_handshake_never_reports_success() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            // Read whatever the client sends (its ClientHello) and reply
            // with a handful of bytes that look like the start of a
            // ServerHello record but stop well short of a complete one,
            // then close -- exactly what a connection reset partway
            // through the first flight looks like.
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(&[0x16, 0x03, 0x03, 0x00, 0x7a, 0x02, 0x00, 0x00]);
        }
    });
    let mut stream = connect(port);
    let config = ClientConfig::new(test_trust_store());
    let mut conn = new_conn(config, "leaf.otter-test.example", common::NOW_NORMAL);

    common::flush(&mut conn, &mut stream);
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => conn.read_tls(&buf[..n]),
            Err(_) => break,
        }
    }
    // However much of the truncated flight arrived, the sans-I/O state
    // machine must simply report "no progress possible yet" -- never a
    // panic, and never a false HandshakeComplete.
    assert!(matches!(conn.process(), Ok(None)), "a truncated handshake must not fabricate progress");
    assert!(!conn.is_closed());
}

/// A scripted fake server: reads (and discards) whatever the client sends,
/// then replies with a hand-built `ServerHello` naming a cipher suite this
/// client never offered (`TLS_RSA_WITH_AES_128_CBC_SHA`, a TLS 1.2-only
/// suite number, 0x002f) but otherwise well-formed (correct
/// `supported_versions`, the client's own echoed session id, a valid-looking
/// x25519 `key_share`) -- every field the brief's ServerHello-validation
/// order checks *before* the cipher suite is fine, so this exercises that
/// specific check.
fn build_server_hello_with_bogus_suite(client_hello: &[u8]) -> Vec<u8> {
    // Pull the client's legacy_session_id (offset 2 (version) + 32 (random) = 34) out of its ClientHello.
    let session_id_len = client_hello[4 + 34] as usize;
    let session_id = &client_hello[4 + 35..4 + 35 + session_id_len];

    let mut body = Vec::new();
    body.extend_from_slice(&0x0303u16.to_be_bytes());
    body.extend_from_slice(&[0x02u8; 32]); // random (not the HelloRetryRequest sentinel)
    body.push(session_id.len() as u8);
    body.extend_from_slice(session_id);
    body.extend_from_slice(&0x002fu16.to_be_bytes()); // bogus, never-offered suite
    body.push(0); // legacy_compression_method

    let mut extensions = Vec::new();
    extensions.extend_from_slice(&43u16.to_be_bytes()); // supported_versions
    extensions.extend_from_slice(&2u16.to_be_bytes());
    extensions.extend_from_slice(&0x0304u16.to_be_bytes());
    extensions.extend_from_slice(&51u16.to_be_bytes()); // key_share
    extensions.extend_from_slice(&36u16.to_be_bytes());
    extensions.extend_from_slice(&0x001du16.to_be_bytes());
    extensions.extend_from_slice(&32u16.to_be_bytes());
    extensions.extend_from_slice(&[0x07u8; 32]);

    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(&extensions);

    let mut message = Vec::new();
    message.push(2); // ServerHello
    let len = (body.len() as u32).to_be_bytes();
    message.extend_from_slice(&len[1..]);
    message.extend_from_slice(&body);

    let mut record = Vec::new();
    record.extend_from_slice(&[0x16, 0x03, 0x03]);
    record.extend_from_slice(&(message.len() as u16).to_be_bytes());
    record.extend_from_slice(&message);
    record
}

/// Reads exactly one TLS record's worth of bytes (header + declared length)
/// from `sock`, looping as needed -- a `ClientHello` is small enough to
/// usually arrive in a single `read`, but this must not assume that.
fn read_one_record(sock: &mut TcpStream) -> Vec<u8> {
    let mut header = [0u8; 5];
    sock.read_exact(&mut header).expect("read record header");
    let len = u16::from_be_bytes([header[3], header[4]]) as usize;
    let mut body = vec![0u8; len];
    sock.read_exact(&mut body).expect("read record body");
    body
}

#[test]
fn server_selecting_an_unoffered_suite_is_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok(mut sock) = listener.accept().map(|(s, _)| s) {
            // The ClientHello's handshake header (type + u24 length) is
            // right at the start of the record body.
            let client_hello = read_one_record(&mut sock);
            let reply = build_server_hello_with_bogus_suite(&client_hello);
            let _ = sock.write_all(&reply);
        }
    });
    let mut stream = connect(port);
    let config = ClientConfig::new(test_trust_store());
    let mut conn = new_conn(config, "leaf.otter-test.example", common::NOW_NORMAL);

    let err = pump_until_event(&mut conn, &mut stream).expect_err("an unoffered cipher suite must be rejected");
    assert!(matches!(err, TlsError::IllegalParameter(_)), "got {err:?}");
}
