use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use otter_claude::Transport;
use otter_http::{Request, ResponseParser, ResponseEvent};
use otter_tls::{ClientConnection, ClientConfig, Event};
use otter_x509::TrustStore;

use crate::{Backend, Connector, AgentError};

/// The HTTP/1.1 + TLS transport for the Claude API.
pub struct AgentTransport<'a> {
    backend: Backend,
    connector: &'a mut dyn Connector,
    rng: &'a mut dyn otter_tls::Rng,
    error_cell: Rc<RefCell<Option<AgentError>>>,
}

impl<'a> AgentTransport<'a> {
    /// Create a new transport for the backend.
    pub fn new(backend: Backend, connector: &'a mut dyn Connector, rng: &'a mut dyn otter_tls::Rng, error_cell: Rc<RefCell<Option<AgentError>>>) -> Self {
        AgentTransport {
            backend,
            connector,
            rng,
            error_cell,
        }
    }
}

impl<'a> Transport for AgentTransport<'a> {
    fn send(&mut self, request: &Request) -> Result<Vec<Vec<u8>>, String> {
        // 1. Create a TLS connection
        let mut stream = match self.connector.connect(&self.backend.host, self.backend.port) {
            Ok(s) => s,
            Err(e) => {
                let err = AgentError::Connect(e.clone());
                *self.error_cell.borrow_mut() = Some(err);
                return Err(format!("connect:{}", e));
            }
        };

        // 2. Build TrustStore from roots_der
        let roots_refs: Vec<&[u8]> = self.backend.roots_der.iter().map(|v| v.as_slice()).collect();
        let trust_store = TrustStore::from_der_roots(roots_refs.as_slice());

        // 3. Create the TLS connection
        let config = ClientConfig::new(trust_store);
        let mut tls_conn = match ClientConnection::new(config, &self.backend.server_name, self.backend.now_unix, &mut *self.rng) {
            Ok(conn) => conn,
            Err(e) => {
                let err_msg = format!("{:?}", e);
                let err = AgentError::Tls(err_msg.clone());
                *self.error_cell.borrow_mut() = Some(err);
                return Err(format!("tls:{}", err_msg));
            }
        };

        // 4. Complete the TLS handshake
        loop {
            // Flush outgoing bytes
            let outgoing = tls_conn.take_outgoing();
            if !outgoing.is_empty() {
                stream.write_all(&outgoing)
                    .map_err(|e| format!("Write error: {}", e))?;
            }

            // Process any pending events
            match tls_conn.process() {
                Ok(Some(Event::HandshakeComplete { .. })) => {
                    // Handshake complete
                    break;
                }
                Ok(Some(Event::Alert { .. })) | Ok(Some(Event::PeerClosed)) => {
                    let err = AgentError::Tls("Peer closed during handshake".to_string());
                    *self.error_cell.borrow_mut() = Some(err);
                    return Err("tls:Peer closed during handshake".to_string());
                }
                Ok(Some(_)) => {} // Other events are ignored during handshake
                Ok(None) => {
                    // Need more input
                    let mut buf = [0u8; 4096];
                    match stream.read(&mut buf) {
                        Ok(0) => return Err("Connection closed by peer".to_string()),
                        Ok(n) => tls_conn.read_tls(&buf[..n]),
                        Err(e) => return Err(format!("Read error: {}", e)),
                    }
                }
                Err(e) => {
                    let err_msg = format!("{:?}", e);
                    let err = AgentError::Tls(err_msg.clone());
                    *self.error_cell.borrow_mut() = Some(err);
                    return Err(format!("tls:{}", err_msg));
                }
            }
        }

        // 5. Serialize the HTTP request
        let http_request = serialize_request(request, &self.backend)?;

        // 6. Send the HTTP request
        tls_conn.send(&http_request)
            .map_err(|e| format!("TLS send error: {:?}", e))?;

        let outgoing = tls_conn.take_outgoing();
        if !outgoing.is_empty() {
            stream.write_all(&outgoing)
                .map_err(|e| format!("Write error: {}", e))?;
        }

        // 7. Read the HTTP response
        let mut all_response_data = Vec::new();

        loop {
            // Try to process more from the TLS connection
            match tls_conn.process() {
                Ok(Some(Event::ApplicationData(data))) => {
                    all_response_data.extend_from_slice(&data);
                }
                Ok(Some(Event::PeerClosed)) => {
                    // Peer closed the connection; we're done
                    break;
                }
                Ok(Some(Event::Alert { .. })) => {
                    return Err("tls:Alert received from peer".to_string());
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    // Need more data from the socket
                    let mut buf = [0u8; 4096];
                    match stream.read(&mut buf) {
                        Ok(0) => {
                            // Connection closed
                            break;
                        }
                        Ok(n) => {
                            tls_conn.read_tls(&buf[..n]);
                        }
                        Err(e) => {
                            return Err(format!("Read error: {}", e));
                        }
                    }
                }
                Err(e) => {
                    return Err(format!("tls:{:?}", e));
                }
            }
        }

        // Parse the HTTP response and extract the body
        let mut parser = ResponseParser::new();
        parser.feed(&all_response_data);

        let mut status_code = 200u16;
        let mut body_chunks = Vec::new();

        loop {
            match parser.next_event() {
                Ok(Some(ResponseEvent::Status(s))) => {
                    status_code = s.code;
                }
                Ok(Some(ResponseEvent::HeadersComplete)) => {
                    // Headers are done, body data will follow
                }
                Ok(Some(ResponseEvent::Data(data))) => {
                    body_chunks.push(data);
                }
                Ok(Some(ResponseEvent::Complete)) => {
                    // Response is complete
                    break;
                }
                Ok(Some(_)) => {
                    // Ignore other events (headers, etc)
                }
                Ok(None) => {
                    // No more events
                    break;
                }
                Err(_e) => {
                    // Parse error, but we might still have body data
                    break;
                }
            }
        }

        // Check status code and handle errors
        if status_code != 200 {
            let body_bytes = body_chunks.concat();
            if let Ok(body_str) = String::from_utf8(body_bytes)
                && let Ok(error_obj) = otter_json::parse_str(&body_str)
                && let Some(msg) = error_obj
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
            {
                return Err(format!("api:{}", msg));
            }
            return Err(format!("http:{}", status_code));
        }

        Ok(body_chunks)
    }
}

/// Serialize the HTTP request into bytes.
fn serialize_request(request: &Request, backend: &Backend) -> Result<Vec<u8>, String> {

    let mut output = Vec::new();

    // Status line
    let method = request.method.as_str();
    let path = if request.url.path.is_empty() { "/" } else { &request.url.path };

    let status_line = format!("{} {} HTTP/1.1\r\n", method, path);
    output.extend_from_slice(status_line.as_bytes());

    // Add Host header (required by HTTP/1.1)
    let host_header = format!("Host: {}\r\n", backend.server_name);
    output.extend_from_slice(host_header.as_bytes());

    // Add other headers
    for (name, value) in &request.headers {
        let header = format!("{}: {}\r\n", name, value);
        output.extend_from_slice(header.as_bytes());
    }

    // Add Content-Length if there's a body
    if !request.body.is_empty() {
        let content_length = format!("Content-Length: {}\r\n", request.body.len());
        output.extend_from_slice(content_length.as_bytes());
    }

    // Add Connection: close header
    output.extend_from_slice(b"Connection: close\r\n");

    // End of headers
    output.extend_from_slice(b"\r\n");

    // Body
    output.extend_from_slice(&request.body);

    Ok(output)
}
