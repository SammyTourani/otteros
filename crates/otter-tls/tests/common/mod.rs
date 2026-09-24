//! Shared helpers for otter-tls's integration tests: a real-entropy [`OsRng`]
//! (this crate's own `Rng` trait has no built-in implementation -- brief
//! M8-T5 has the caller supply one, which in production will be the kernel;
//! here it is a thin `/dev/urandom` reader, std-only and test-only), PKI
//! fixture loading (brief M8-T4's `otter-x509` test PKI, reused rather than
//! duplicated -- see `scripts/tls-gen-server-pem.sh`), an `openssl s_server`
//! process guard, and the sans-I/O pump loop every test drives a
//! [`otter_tls::ClientConnection`] with over a plain `std::net::TcpStream`.
#![allow(dead_code)] // not every test file uses every helper.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use otter_tls::{ClientConnection, Event, Rng, TlsError};

pub const OPENSSL: &str = "/opt/homebrew/bin/openssl";

/// Reads real randomness from `/dev/urandom` -- available on every macOS
/// (and Linux) host this crate's tests run on; this is test-only code, not
/// part of the `no_std` crate itself (brief M8-T5: "the caller supplies
/// randomness through a trait").
pub struct OsRng(std::fs::File);

impl OsRng {
    pub fn new() -> OsRng {
        OsRng(std::fs::File::open("/dev/urandom").expect("open /dev/urandom"))
    }
}

impl Rng for OsRng {
    fn fill(&mut self, dest: &mut [u8]) {
        self.0.read_exact(dest).expect("read from /dev/urandom");
    }
}

pub fn otter_x509_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../otter-x509/tests/fixtures")
}

pub fn server_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server")
}

/// Reads one `scripts/make-test-pki.sh`-generated DER certificate by name
/// (without the `.der` extension).
pub fn pki_der(name: &str) -> Vec<u8> {
    let path = otter_x509_fixtures_dir().join("pki").join(format!("{name}.der"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("could not read {path:?} ({e}); run scripts/make-test-pki.sh first"))
}

/// A `TrustStore` containing only brief M8-T4's test root -- what the local
/// `openssl s_server` interop tests validate against.
pub fn test_trust_store() -> otter_x509::TrustStore {
    let root = pki_der("root");
    otter_x509::TrustStore::from_der_roots(&[&root])
}

/// A Unix time within the test PKI's normal validity window
/// (2024-01-01..2034-01-01Z), matching `otter-x509`'s own `tests/common`.
pub const NOW_NORMAL: u64 = 1_748_736_000;

/// A Unix time after every test-PKI certificate's 2034-01-01Z `notAfter`
/// (and after `leaf_expired`'s own 2020-02-01Z `notAfter`).
pub const AFTER_ALL_EXPIRED: u64 = 2_051_308_800;

/// Finds a currently-free TCP port on localhost by briefly binding to port 0.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port").local_addr().expect("local_addr").port()
}

/// A running `openssl s_server`, killed when dropped.
pub struct OpensslServer {
    child: Child,
}

impl Drop for OpensslServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts `openssl s_server -www` on `port` presenting `leaf_pem`/`key_pem`
/// plus `intermediate_pem` (via `-cert_chain`: `-cert` alone sends only the
/// first certificate in the file, even when it is handed a multi-certificate
/// bundle), restricted to TLS 1.3 and (if given) a single ciphersuite, and
/// waits for it to start accepting connections. `-www` (rather than a
/// hand-rolled echo) gives every test a real, deterministic HTTP-shaped
/// response to exercise a genuine bidirectional application-data exchange
/// against.
pub fn start_s_server(port: u16, leaf_pem: &PathBuf, key_pem: &PathBuf, intermediate_pem: &PathBuf, ciphersuite: Option<&str>) -> OpensslServer {
    let mut cmd = Command::new(OPENSSL);
    cmd.arg("s_server")
        .arg("-accept")
        .arg(port.to_string())
        .arg("-cert")
        .arg(leaf_pem)
        .arg("-cert_chain")
        .arg(intermediate_pem)
        .arg("-key")
        .arg(key_pem)
        .arg("-tls1_3")
        .arg("-www")
        .arg("-quiet")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(suite) = ciphersuite {
        cmd.arg("-ciphersuites").arg(suite);
    }
    let child = cmd.spawn().expect("spawn openssl s_server (is /opt/homebrew/bin/openssl installed?)");
    let server = OpensslServer { child };
    wait_for_port(port);
    server
}

fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if Instant::now() > deadline {
            panic!("openssl s_server on port {port} never started accepting connections");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Connects a plain TCP socket to `127.0.0.1:port` with a generous read
/// timeout (QEMU-free host tests, but a hung peer must still fail fast
/// rather than wedging the whole suite).
pub fn connect(port: u16) -> TcpStream {
    let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to 127.0.0.1");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("set_read_timeout");
    stream.set_nodelay(true).expect("set_nodelay");
    stream
}

/// Drives `conn` over `stream` (writing queued outgoing bytes, reading and
/// feeding back whatever arrives) until [`ClientConnection::process`]
/// yields an `Event` or a local/remote failure, whichever comes first.
/// Panics on a raw I/O error or a peer that closes the TCP connection
/// without ever producing an event (a plain read timeout is such an I/O
/// error) -- appropriate for the "this must succeed" interop tests; tests
/// that specifically want to observe truncation drive the loop by hand.
pub fn pump_until_event(conn: &mut ClientConnection, stream: &mut TcpStream) -> Result<Event, TlsError> {
    loop {
        flush(conn, stream);
        if let Some(event) = conn.process()? {
            // `process` can itself queue bytes as a side effect of the very
            // event it just returned (e.g. the client's Finished, queued
            // while producing HandshakeComplete) -- flush those before
            // handing control back, or they would sit buffered until some
            // unrelated later call happened to flush them.
            flush(conn, stream);
            return Ok(event);
        }
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).expect("read from socket (timed out or the peer reset the connection)");
        if n == 0 {
            panic!("peer closed the TCP connection without completing the handshake or sending an alert");
        }
        conn.read_tls(&buf[..n]);
    }
}

/// Writes any bytes [`ClientConnection::take_outgoing`] currently has
/// queued, if any.
pub fn flush(conn: &mut ClientConnection, stream: &mut TcpStream) {
    let out = conn.take_outgoing();
    if !out.is_empty() {
        stream.write_all(&out).expect("write to socket");
    }
}

/// Runs the handshake to completion, returning the `(suite, alpn, peer)`
/// tuple from [`Event::HandshakeComplete`] and how long it took.
pub fn run_handshake(conn: &mut ClientConnection, stream: &mut TcpStream) -> (otter_tls::CipherSuite, Option<Vec<u8>>, String, Duration) {
    let start = Instant::now();
    match pump_until_event(conn, stream).expect("handshake failed") {
        Event::HandshakeComplete { suite, alpn, peer } => (suite, alpn, peer, start.elapsed()),
        other => panic!("expected HandshakeComplete, got {other:?}"),
    }
}
