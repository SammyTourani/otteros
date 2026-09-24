//! Local interop (brief M8-T5): a real handshake against `openssl s_server`
//! (Homebrew OpenSSL 3.6 CLI) using brief M8-T4's test PKI, forcing each of
//! this client's three cipher suites in turn, then a small HTTP exchange
//! (`openssl s_server -www`) to prove application data flows both ways.

mod common;

use std::io::Read;
use std::time::Duration;

use common::{OsRng, connect, run_handshake, server_fixtures_dir, start_s_server, test_trust_store};
use otter_tls::{CipherSuite, ClientConfig, ClientConnection, Event};

/// Runs a full handshake plus one HTTP/1.0 request/response against a fresh
/// `openssl s_server` forced to `ciphersuite`, and asserts this client
/// negotiated exactly the suite that was offered.
fn interop_one_suite(ciphersuite: &str, expected: CipherSuite) {
    let port = common::free_port();
    let fixtures = server_fixtures_dir();
    let _server = start_s_server(port, &fixtures.join("leaf.pem"), &fixtures.join("leaf.key"), &fixtures.join("intermediate.pem"), Some(ciphersuite));
    let mut stream = connect(port);

    let config = ClientConfig::new(test_trust_store());
    let mut conn = ClientConnection::new(config, "leaf.otter-test.example", common::NOW_NORMAL, &mut OsRng::new()).expect("ClientConnection::new");

    let (suite, alpn, peer, elapsed) = run_handshake(&mut conn, &mut stream);
    assert_eq!(suite, expected, "negotiated a different suite than the one offered");
    assert!(alpn.is_none(), "no ALPN was offered, so none should be negotiated");
    assert!(peer.contains("leaf.otter-test.example"), "unexpected peer display name: {peer}");
    eprintln!("local interop: {ciphersuite:?} handshake completed in {elapsed:?}");

    conn.send(b"GET / HTTP/1.0\r\n\r\n").expect("send");
    let response = read_application_data_until_closed(&mut conn, &mut stream);
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.0 200 ok"), "unexpected response: {response}");

    conn.close();
    common::flush(&mut conn, &mut stream);
}

/// Drives `conn` until the peer signals `close_notify` (or the TCP
/// connection simply ends, which `openssl s_server -www` also does after
/// answering), collecting every `ApplicationData` chunk along the way.
fn read_application_data_until_closed(conn: &mut ClientConnection, stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    loop {
        common::flush(conn, stream);
        match conn.process().expect("process") {
            Some(Event::ApplicationData(chunk)) => data.extend_from_slice(&chunk),
            Some(Event::PeerClosed) => return data,
            Some(other) => panic!("unexpected event while reading the response: {other:?}"),
            None => {
                let mut buf = [0u8; 8192];
                match stream.read(&mut buf) {
                    Ok(0) => return data, // openssl s_server -www closes the TCP connection outright.
                    Ok(n) => conn.read_tls(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => panic!("socket read error: {e}"),
                }
            }
        }
    }
}

#[test]
fn aes_128_gcm_sha256() {
    interop_one_suite("TLS_AES_128_GCM_SHA256", CipherSuite::Aes128GcmSha256);
}

#[test]
fn aes_256_gcm_sha384() {
    interop_one_suite("TLS_AES_256_GCM_SHA384", CipherSuite::Aes256GcmSha384);
}

#[test]
fn chacha20_poly1305_sha256() {
    interop_one_suite("TLS_CHACHA20_POLY1305_SHA256", CipherSuite::Chacha20Poly1305Sha256);
}

/// ALPN negotiation against `openssl s_server -alpn`.
#[test]
fn alpn_is_negotiated() {
    let port = common::free_port();
    let fixtures = server_fixtures_dir();
    let mut cmd = std::process::Command::new(common::OPENSSL);
    cmd.arg("s_server")
        .arg("-accept")
        .arg(port.to_string())
        .arg("-cert")
        .arg(fixtures.join("leaf.pem"))
        .arg("-cert_chain")
        .arg(fixtures.join("intermediate.pem"))
        .arg("-key")
        .arg(fixtures.join("leaf.key"))
        .arg("-tls1_3")
        .arg("-www")
        .arg("-alpn")
        .arg("http/1.1")
        .arg("-quiet")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().expect("spawn openssl s_server");
    std::thread::sleep(Duration::from_millis(300));
    let mut stream = connect(port);

    let config = ClientConfig::new(test_trust_store()).with_alpn_protocols(vec![b"http/1.1".to_vec()]);
    let mut conn = ClientConnection::new(config, "leaf.otter-test.example", common::NOW_NORMAL, &mut OsRng::new()).expect("ClientConnection::new");
    let (_, alpn, _, _) = run_handshake(&mut conn, &mut stream);
    assert_eq!(alpn, Some(b"http/1.1".to_vec()));

    let _ = child.kill();
    let _ = child.wait();
}
