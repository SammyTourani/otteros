use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::thread;
use std::time::Duration;

use otter_http::{Request, ResponseParser, ResponseEvent, Url};

/// Start the test HTTP server on a random port
#[allow(clippy::zombie_processes)]
fn start_test_server() -> (u16, Child) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let child = Command::new("python3")
        .arg("scripts/http-test-server.py")
        .arg(port.to_string())
        .current_dir("../..")
        .spawn()
        .expect("Failed to start test server");

    for _ in 0..50 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return (port, child);
        }
        thread::sleep(Duration::from_millis(100));
    }

    panic!("Test server did not start in time");
}

struct TcpTransport {
    stream: TcpStream,
}

impl TcpTransport {
    fn new(addr: &str) -> std::io::Result<Self> {
        Ok(TcpTransport {
            stream: TcpStream::connect(addr)?,
        })
    }

    fn send_request(&mut self, request: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(request)?;
        self.stream.flush()
    }

    fn read_response(&mut self) -> std::io::Result<Vec<u8>> {
        let mut response = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        Ok(response)
    }
}

#[test]
fn test_http_fixed_length() {
    let (port, mut server) = start_test_server();

    let url = Url::parse(&format!("http://127.0.0.1:{}/fixed", port)).expect("parse URL");
    let request = Request::get(url);
    let request_bytes = request.serialize().expect("serialize");

    let mut transport = TcpTransport::new(&format!("127.0.0.1:{}", port)).expect("connect");
    transport.send_request(&request_bytes).expect("send");

    let response_bytes = transport.read_response().expect("read");

    let mut parser = ResponseParser::new();
    parser.feed(&response_bytes);

    let mut body = Vec::new();
    while let Ok(Some(event)) = parser.next_event() {
        match event {
            ResponseEvent::Data(d) => body.extend_from_slice(&d),
            ResponseEvent::Complete => break,
            _ => {}
        }
    }

    assert_eq!(body, b"Hello, World!");
    let _ = server.kill();
}

#[test]
fn test_http_chunked() {
    let (port, mut server) = start_test_server();

    let url = Url::parse(&format!("http://127.0.0.1:{}/chunked", port)).expect("parse URL");
    let request = Request::get(url);
    let request_bytes = request.serialize().expect("serialize");

    let mut transport = TcpTransport::new(&format!("127.0.0.1:{}", port)).expect("connect");
    transport.send_request(&request_bytes).expect("send");

    let response_bytes = transport.read_response().expect("read");

    let mut parser = ResponseParser::new();
    parser.feed(&response_bytes);

    let mut body = Vec::new();
    while let Ok(Some(event)) = parser.next_event() {
        match event {
            ResponseEvent::Data(d) => body.extend_from_slice(&d),
            ResponseEvent::Complete => break,
            _ => {}
        }
    }

    assert_eq!(body, b"Hello, World!");
    let _ = server.kill();
}

#[test]
fn test_http_redirect() {
    let (port, mut server) = start_test_server();

    let url = Url::parse(&format!("http://127.0.0.1:{}/redirect-301", port)).expect("parse URL");
    let request = Request::get(url);
    let request_bytes = request.serialize().expect("serialize");

    let mut transport = TcpTransport::new(&format!("127.0.0.1:{}", port)).expect("connect");
    transport.send_request(&request_bytes).expect("send");

    let response_bytes = transport.read_response().expect("read");

    let mut parser = ResponseParser::new();
    parser.feed(&response_bytes);

    let mut status = None;
    let mut location = None;
    while let Ok(Some(event)) = parser.next_event() {
        match event {
            ResponseEvent::Status(s) => {
                status = Some(s);
            }
            ResponseEvent::Header(h) if h.name.to_lowercase() == "location" => {
                location = Some(h.value);
            }
            _ => {}
        }
    }

    assert_eq!(status.unwrap().code, 301);
    assert_eq!(location.unwrap(), "/redirected");
    let _ = server.kill();
}

#[test]
fn test_http_204() {
    let (port, mut server) = start_test_server();

    let url = Url::parse(&format!("http://127.0.0.1:{}/204", port)).expect("parse URL");
    let request = Request::get(url);
    let request_bytes = request.serialize().expect("serialize");

    let mut transport = TcpTransport::new(&format!("127.0.0.1:{}", port)).expect("connect");
    transport.send_request(&request_bytes).expect("send");

    let response_bytes = transport.read_response().expect("read");

    let mut parser = ResponseParser::new();
    parser.feed(&response_bytes);

    let mut status = None;
    while let Ok(Some(event)) = parser.next_event() {
        match event {
            ResponseEvent::Status(s) => {
                status = Some(s);
            }
            ResponseEvent::Complete => break,
            _ => {}
        }
    }

    assert_eq!(status.unwrap().code, 204);
    assert!(parser.is_complete());
    let _ = server.kill();
}
