//! Live interop against real public servers (brief M8-T5). `#[ignore]`:
//! run explicitly with `-- --ignored` (this is what the brief's acceptance
//! commands do); skips cleanly (rather than failing) when this machine is
//! offline. Reports the negotiated suite and handshake time for each host.
//!
//! Scope, per the brief and the orchestrator's network allow-list for this
//! task: a full handshake plus one `GET / HTTP/1.1` to example.com,
//! www.cloudflare.com, www.google.com and github.com; a handshake only (no
//! request at all) to api.anthropic.com.

mod common;

use std::io::Read;
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{OsRng, pump_until_event};
use otter_tls::{ClientConfig, ClientConnection, Event};

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("system clock is before 1970").as_secs()
}

/// Connects to `host:443`, or panics with a message `#[ignore]`'d live
/// tests are expected to be read by a human (this is not treated as
/// "offline" -- a DNS/connect failure to a real, normally-reachable public
/// host is worth seeing, not silently swallowed like a missing network
/// route is, below).
fn connect(host: &str) -> Option<TcpStream> {
    match TcpStream::connect((host, 443)) {
        Ok(stream) => {
            stream.set_read_timeout(Some(Duration::from_secs(15))).expect("set_read_timeout");
            stream.set_nodelay(true).expect("set_nodelay");
            Some(stream)
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NetworkUnreachable | std::io::ErrorKind::TimedOut | std::io::ErrorKind::HostUnreachable | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            eprintln!("skipping live test against {host}: appears to be offline ({e})");
            None
        }
        Err(e) => panic!("connect to {host}:443 failed: {e}"),
    }
}

fn handshake_and_get(host: &str) {
    let Some(mut stream) = connect(host) else { return };
    let config = ClientConfig::new(otter_x509::roots::trust_store());
    let mut conn = ClientConnection::new(config, host, now_unix(), &mut OsRng::new()).expect("ClientConnection::new");

    let start = std::time::Instant::now();
    let (suite, _alpn, peer) = match pump_until_event(&mut conn, &mut stream).expect("handshake failed") {
        Event::HandshakeComplete { suite, alpn, peer } => (suite, alpn, peer),
        other => panic!("expected HandshakeComplete, got {other:?}"),
    };
    let elapsed = start.elapsed();
    println!("live interop: {host} -> suite {suite:?}, handshake in {elapsed:?}, peer {peer:?}");

    let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    conn.send(request.as_bytes()).expect("send");

    let mut response = Vec::new();
    loop {
        common::flush(&mut conn, &mut stream);
        match conn.process().expect("process") {
            Some(Event::ApplicationData(chunk)) => response.extend_from_slice(&chunk),
            Some(Event::PeerClosed) => break,
            Some(Event::Alert { level, description }) => {
                println!("live interop: {host} sent alert {level:?}/{description:?} after the response");
                break;
            }
            Some(other) => panic!("unexpected event from {host}: {other:?}"),
            None => {
                let mut buf = [0u8; 8192];
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => conn.read_tls(&buf[..n]),
                    Err(e) => panic!("socket read error from {host}: {e}"),
                }
            }
        }
        if response.len() > 1 << 20 {
            break; // more than enough to have seen a status line; do not download an entire page.
        }
    }
    conn.close();
    common::flush(&mut conn, &mut stream);

    let text = String::from_utf8_lossy(&response);
    let status_line = text.lines().next().unwrap_or_default();
    assert!(status_line.starts_with("HTTP/1."), "unexpected response from {host}: {status_line:?}");
    println!("live interop: {host} -> {status_line}");
}

#[test]
#[ignore]
fn example_com() {
    handshake_and_get("example.com");
}

#[test]
#[ignore]
fn www_cloudflare_com() {
    handshake_and_get("www.cloudflare.com");
}

#[test]
#[ignore]
fn www_google_com() {
    handshake_and_get("www.google.com");
}

#[test]
#[ignore]
fn github_com() {
    handshake_and_get("github.com");
}

/// Handshake only -- no request is ever sent, per this task's network
/// allow-list.
#[test]
#[ignore]
fn api_anthropic_com_handshake_only() {
    let Some(mut stream) = connect("api.anthropic.com") else { return };
    let config = ClientConfig::new(otter_x509::roots::trust_store());
    let mut conn = ClientConnection::new(config, "api.anthropic.com", now_unix(), &mut OsRng::new()).expect("ClientConnection::new");

    let start = std::time::Instant::now();
    match pump_until_event(&mut conn, &mut stream).expect("handshake failed") {
        Event::HandshakeComplete { suite, peer, .. } => {
            println!("live interop: api.anthropic.com -> suite {suite:?}, handshake in {:?}, peer {peer:?}", start.elapsed());
        }
        other => panic!("expected HandshakeComplete, got {other:?}"),
    }

    conn.close();
    common::flush(&mut conn, &mut stream);
}
