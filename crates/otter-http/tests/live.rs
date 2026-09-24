use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{SystemTime, UNIX_EPOCH};

use otter_http::{ResponseParser, ResponseEvent};
use otter_tls::{ClientConfig, ClientConnection, Event, Rng};

/// Get current Unix timestamp
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before 1970")
        .as_secs()
}

/// Simple RNG for TLS
struct TestRng {
    counter: u64,
}

impl TestRng {
    fn new() -> Self {
        TestRng { counter: 0 }
    }
}

impl Rng for TestRng {
    fn fill(&mut self, dest: &mut [u8]) {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hash, Hasher};

        // Not cryptographically secure, but fine for tests
        for chunk in dest.chunks_mut(8) {
            let mut hasher = RandomState::new().build_hasher();
            self.counter.hash(&mut hasher);
            std::time::SystemTime::now().hash(&mut hasher);
            let bits = hasher.finish().to_le_bytes();
            let len = chunk.len().min(8);
            chunk[..len].copy_from_slice(&bits[..len]);
            self.counter += 1;
        }
    }
}

/// Connect to a host via TLS (returns (status_code, body))
fn fetch_https(host: &str) -> Option<(u16, Vec<u8>)> {
    // Connect via TCP
    let mut stream = match TcpStream::connect((host, 443)) {
        Ok(s) => {
            s.set_read_timeout(Some(std::time::Duration::from_secs(15)))
                .ok()?;
            s.set_nodelay(true).ok()?;
            s
        }
        Err(e) => {
            eprintln!("Connection failed to {}: {}", host, e);
            return None;
        }
    };

    // Setup TLS
    let config = ClientConfig::new(otter_x509::roots::trust_store());
    let mut rng = TestRng::new();
    let mut tls_conn = ClientConnection::new(config, host, now_unix(), &mut rng).ok()?;

    // Perform TLS handshake
    loop {
        match tls_conn.process().ok()? {
            Some(Event::HandshakeComplete { .. }) => break,
            Some(Event::ApplicationData(_)) => continue, // Shouldn't happen during handshake
            Some(_) => continue,
            None => {
                // Need more data
                let mut buf = [0u8; 8192];
                match stream.read(&mut buf) {
                    Ok(0) => return None,
                    Ok(n) => tls_conn.read_tls(&buf[..n]),
                    Err(_) => return None,
                }
            }
        }
    }

    // Send HTTP request
    let request = format!("GET / HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", host);
    tls_conn.send(request.as_bytes()).ok()?;

    // Flush
    loop {
        match tls_conn.take_outgoing() {
            outgoing if !outgoing.is_empty() => {
                stream.write_all(&outgoing).ok()?;
            }
            _ => break,
        }
    }

    // Read response
    let mut response_data = Vec::new();
    loop {
        match tls_conn.process().ok()? {
            Some(Event::ApplicationData(chunk)) => response_data.extend_from_slice(&chunk),
            Some(Event::PeerClosed) => break,
            Some(_) => continue,
            None => {
                let mut buf = [0u8; 8192];
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => tls_conn.read_tls(&buf[..n]),
                    Err(_) => break,
                }
            }
        }

        if response_data.len() > 1 << 20 {
            break; // Stop after 1 MB
        }
    }

    // Parse HTTP response
    let mut parser = ResponseParser::new();
    parser.feed(&response_data);

    let mut status_code = 0;
    let mut body = Vec::new();

    while let Ok(Some(event)) = parser.next_event() {
        match event {
            ResponseEvent::Status(s) => status_code = s.code,
            ResponseEvent::Data(d) => body.extend_from_slice(&d),
            ResponseEvent::Complete => break,
            _ => {}
        }
    }

    if status_code > 0 {
        Some((status_code, body))
    } else {
        None
    }
}

#[test]
#[ignore]
fn test_example_com_https() {
    let (status, body) = match fetch_https("example.com") {
        Some(result) => result,
        None => {
            eprintln!("Could not connect to example.com; assuming offline");
            return;
        }
    };

    assert_eq!(status, 200, "Expected 200 status from example.com");

    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("Example Domain"),
        "Expected body to contain 'Example Domain', got: {}",
        &body_str[..200.min(body_str.len())]
    );

    println!("✓ example.com returned 200 with correct body content");
}

#[test]
#[ignore]
fn test_github_com_redirect() {
    // Test HTTP -> HTTPS redirect
    let mut stream = match TcpStream::connect(("github.com", 80)) {
        Ok(s) => {
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(15)));
            s
        }
        Err(e) => {
            eprintln!("Could not connect to github.com:80; assuming offline: {}", e);
            return;
        }
    };

    let request = "GET / HTTP/1.1\r\nHost: github.com\r\nConnection: close\r\n\r\n";
    if stream.write_all(request.as_bytes()).is_err() {
        eprintln!("Failed to send request to github.com");
        return;
    }

    let mut response = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }

    let mut parser = ResponseParser::new();
    parser.feed(&response);

    let mut status_code = 0;
    let mut location = None;

    while let Ok(Some(event)) = parser.next_event() {
        match event {
            ResponseEvent::Status(s) => status_code = s.code,
            ResponseEvent::Header(h) if h.name.to_lowercase() == "location" => {
                location = Some(h.value);
            }
            _ => {}
        }
    }

    assert!(
        status_code == 301 || status_code == 302 || status_code == 307 || status_code == 308,
        "Expected redirect status, got {}",
        status_code
    );
    assert!(
        location.is_some(),
        "Expected Location header in redirect response"
    );
    assert!(
        location.as_ref().unwrap().contains("https"),
        "Expected redirect to HTTPS"
    );

    println!("✓ github.com HTTP -> HTTPS redirect chain: {} -> {}", status_code, location.unwrap());
}
