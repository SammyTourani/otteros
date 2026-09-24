use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::{HttpError, MAX_HEADER_SIZE, MAX_LINE_SIZE};

/// HTTP response header
#[derive(Debug, Clone)]
pub struct Header {
    /// Header name
    pub name: String,
    /// Header value
    pub value: String,
}

/// HTTP response status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// HTTP version (e.g., 11 for HTTP/1.1)
    pub version: u16,
    /// Status code
    pub code: u16,
}

impl Status {
    /// Check if this is a 204 No Content response
    pub fn is_no_content(&self) -> bool {
        self.code == 204
    }

    /// Check if this is a 304 Not Modified response
    pub fn is_not_modified(&self) -> bool {
        self.code == 304
    }

    /// Check if status indicates success
    pub fn is_success(&self) -> bool {
        self.code >= 200 && self.code < 300
    }
}

/// HTTP response body encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyEncoding {
    /// Content-Length specified
    ContentLength(usize),
    /// Transfer-Encoding: chunked
    Chunked,
    /// Read until close
    UntilClose,
}

/// HTTP response parser (sans-I/O)
pub struct ResponseParser {
    state: ParserState,
    buffer: Vec<u8>,
    status: Option<Status>,
    headers: Vec<Header>,
    body_encoding: Option<BodyEncoding>,
    chunks_remaining: usize,
    body_size: usize,
    header_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParserState {
    StatusLine,
    Headers,
    Body,
    Chunks,
    ChunkData,
    Trailers,
    Done,
}

/// Response data event
#[derive(Debug, Clone)]
pub enum ResponseEvent {
    /// Status line received
    Status(Status),
    /// Header received
    Header(Header),
    /// Headers complete
    HeadersComplete,
    /// Body data received
    Data(Vec<u8>),
    /// Response complete
    Complete,
}

impl ResponseParser {
    /// Create a new response parser
    pub fn new() -> Self {
        ResponseParser {
            state: ParserState::StatusLine,
            buffer: Vec::new(),
            status: None,
            headers: Vec::new(),
            body_encoding: None,
            chunks_remaining: 0,
            body_size: 0,
            header_size: 0,
        }
    }

    /// Feed data to the parser
    pub fn feed(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    /// Try to parse the next event
    pub fn next_event(&mut self) -> Result<Option<ResponseEvent>, HttpError> {
        loop {
            match self.state {
                ParserState::StatusLine => {
                    if let Some((line, rest)) = find_line(&self.buffer) {
                        let status = parse_status_line(line)?;
                        self.status = Some(status);
                        self.buffer = rest.to_vec();
                        self.state = ParserState::Headers;
                        return Ok(Some(ResponseEvent::Status(status)));
                    } else if self.buffer.len() > MAX_LINE_SIZE {
                        return Err(HttpError::LineTooLong);
                    }
                    return Ok(None);
                }

                ParserState::Headers => {
                    if let Some((line, rest)) = find_line(&self.buffer) {
                        self.header_size += line.len() + 2; // +2 for \r\n

                        if self.header_size > MAX_HEADER_SIZE {
                            return Err(HttpError::HeadersTooLarge);
                        }

                        if line.is_empty() {
                            // Empty line marks end of headers
                            self.buffer = rest.to_vec();

                            // Determine body encoding
                            self.determine_body_encoding()?;

                            self.state = if matches!(self.body_encoding, Some(BodyEncoding::ContentLength(0))) {
                                ParserState::Done
                            } else if matches!(self.body_encoding, Some(BodyEncoding::Chunked)) {
                                ParserState::Chunks
                            } else {
                                ParserState::Body
                            };

                            return Ok(Some(ResponseEvent::HeadersComplete));
                        } else {
                            // Parse header
                            let header = parse_header(line)?;
                            self.headers.push(header.clone());
                            self.buffer = rest.to_vec();
                            return Ok(Some(ResponseEvent::Header(header)));
                        }
                    } else if self.header_size > MAX_HEADER_SIZE {
                        return Err(HttpError::HeadersTooLarge);
                    } else if self.buffer.len() > MAX_LINE_SIZE {
                        return Err(HttpError::LineTooLong);
                    }
                    return Ok(None);
                }

                ParserState::Body => {
                    match self.body_encoding {
                        Some(BodyEncoding::ContentLength(len)) => {
                            let remaining = len.saturating_sub(self.body_size);
                            if remaining == 0 {
                                self.state = ParserState::Done;
                                return Ok(Some(ResponseEvent::Complete));
                            }

                            let to_consume = remaining.min(self.buffer.len());
                            if to_consume > 0 {
                                let data = self.buffer[..to_consume].to_vec();
                                self.buffer.drain(..to_consume);
                                self.body_size += to_consume;
                                return Ok(Some(ResponseEvent::Data(data)));
                            }
                            return Ok(None);
                        }

                        Some(BodyEncoding::UntilClose) => {
                            if !self.buffer.is_empty() {
                                let data = core::mem::take(&mut self.buffer);
                                return Ok(Some(ResponseEvent::Data(data)));
                            }
                            return Ok(None);
                        }

                        _ => return Err(HttpError::InvalidRequest),
                    }
                }

                ParserState::Chunks => {
                    if let Some((line, rest)) = find_line(&self.buffer) {
                        // Parse chunk size
                        let line_str = core::str::from_utf8(line).unwrap_or("");
                        let size_str = line_str.split(';').next().unwrap_or(line_str).trim();
                        self.chunks_remaining = usize::from_str_radix(size_str, 16)
                            .map_err(|_| HttpError::InvalidChunkSize)?;

                        self.buffer = rest.to_vec();

                        if self.chunks_remaining == 0 {
                            // Last chunk, move to trailers
                            self.state = ParserState::Trailers;
                        } else {
                            self.state = ParserState::ChunkData;
                        }
                        // Continue the loop to process the next state
                    } else if self.buffer.len() > MAX_LINE_SIZE {
                        return Err(HttpError::LineTooLong);
                    } else {
                        return Ok(None);
                    }
                }

                ParserState::ChunkData => {
                    if self.buffer.len() >= self.chunks_remaining + 2 {
                        // +2 for trailing \r\n
                        let data = self.buffer[..self.chunks_remaining].to_vec();
                        self.buffer.drain(..self.chunks_remaining + 2); // Remove data and \r\n
                        self.body_size += data.len();
                        self.state = ParserState::Chunks;
                        return Ok(Some(ResponseEvent::Data(data)));
                    }
                    return Ok(None);
                }

                ParserState::Trailers => {
                    // Skip any trailers and look for final \r\n
                    if let Some((line, rest)) = find_line(&self.buffer) {
                        if line.is_empty() {
                            self.buffer = rest.to_vec();
                            self.state = ParserState::Done;
                            return Ok(Some(ResponseEvent::Complete));
                        }
                        // Skip trailer header
                        self.buffer = rest.to_vec();
                    } else if self.buffer.len() > MAX_LINE_SIZE {
                        return Err(HttpError::LineTooLong);
                    }
                    return Ok(None);
                }

                ParserState::Done => {
                    return Ok(None);
                }
            }
        }
    }

    /// Get the current status
    pub fn status(&self) -> Option<Status> {
        self.status
    }

    /// Get a header value (case-insensitive lookup)
    pub fn get_header(&self, name: &str) -> Option<&str> {
        let name_lower = name.to_lowercase();
        self.headers
            .iter()
            .find(|h| h.name.to_lowercase() == name_lower)
            .map(|h| h.value.as_str())
    }

    /// Check if we're done parsing
    pub fn is_complete(&self) -> bool {
        matches!(self.state, ParserState::Done)
    }

    #[allow(clippy::collapsible_if)]
    fn determine_body_encoding(&mut self) -> Result<(), HttpError> {
        // Check for no-body responses
        if let Some(status) = self.status {
            if status.code == 204 || status.code == 304 {
                self.body_encoding = Some(BodyEncoding::ContentLength(0));
                self.state = ParserState::Done;
                return Ok(());
            }
        }

        // Check for Transfer-Encoding: chunked
        if let Some(te) = self.get_header("Transfer-Encoding") {
            if te.to_lowercase().contains("chunked") {
                self.body_encoding = Some(BodyEncoding::Chunked);
                return Ok(());
            }
        }

        // Check for Content-Length
        if let Some(cl) = self.get_header("Content-Length") {
            let len = cl.parse::<usize>()
                .map_err(|_| HttpError::InvalidContentLength)?;
            self.body_encoding = Some(BodyEncoding::ContentLength(len));
            return Ok(());
        }

        // Default to read-until-close
        self.body_encoding = Some(BodyEncoding::UntilClose);
        Ok(())
    }
}

impl Default for ResponseParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Find a line in a buffer (handles \r\n, \n, or \r)
fn find_line(buffer: &[u8]) -> Option<(&[u8], &[u8])> {
    for i in 0..buffer.len() {
        if buffer[i] == b'\n' {
            let line = if i > 0 && buffer[i - 1] == b'\r' {
                &buffer[..i - 1]
            } else {
                &buffer[..i]
            };
            return Some((line, &buffer[i + 1..]));
        } else if buffer[i] == b'\r' && i + 1 < buffer.len() && buffer[i + 1] == b'\n' {
            return Some((&buffer[..i], &buffer[i + 2..]));
        }
    }
    None
}

/// Parse status line (e.g., "HTTP/1.1 200 OK")
fn parse_status_line(line: &[u8]) -> Result<Status, HttpError> {
    let s = core::str::from_utf8(line).map_err(|_| HttpError::InvalidStatus)?;
    let parts: Vec<&str> = s.split_whitespace().collect();

    if parts.len() < 2 {
        return Err(HttpError::InvalidStatus);
    }

    // Parse version (HTTP/1.1 -> 11)
    let version = if parts[0] == "HTTP/1.1" {
        11
    } else if parts[0] == "HTTP/1.0" {
        10
    } else {
        return Err(HttpError::InvalidStatus);
    };

    let code = parts[1].parse::<u16>()
        .map_err(|_| HttpError::InvalidStatus)?;

    Ok(Status { version, code })
}

/// Parse a header line (e.g., "Content-Type: text/html")
fn parse_header(line: &[u8]) -> Result<Header, HttpError> {
    let s = core::str::from_utf8(line).map_err(|_| HttpError::InvalidHeader)?;

    if let Some(colon_pos) = s.find(':') {
        let name = s[..colon_pos].trim().to_string();
        let value = s[colon_pos + 1..].trim().to_string();
        Ok(Header { name, value })
    } else {
        Err(HttpError::InvalidHeader)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_response() {
        let mut parser = ResponseParser::new();
        parser.feed(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello");

        // Status
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::Status(s) => {
                assert_eq!(s.code, 200);
                assert_eq!(s.version, 11);
            }
            _ => panic!("Expected status"),
        }

        // Header
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::Header(h) => {
                assert_eq!(h.name, "Content-Length");
                assert_eq!(h.value, "5");
            }
            _ => panic!("Expected header"),
        }

        // Headers complete
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::HeadersComplete => {}
            _ => panic!("Expected headers complete"),
        }

        // Data
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::Data(d) => {
                assert_eq!(d, b"hello");
            }
            _ => panic!("Expected data"),
        }

        // Complete
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::Complete => {}
            _ => panic!("Expected complete"),
        }
    }

    #[test]
    fn test_parse_chunked_response() {
        let mut parser = ResponseParser::new();
        parser.feed(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        parser.feed(b"5\r\nhello\r\n");
        parser.feed(b"6\r\n world\r\n");
        parser.feed(b"0\r\n\r\n");

        // Status
        let _ = parser.next_event().unwrap();

        // Header
        let _ = parser.next_event().unwrap();

        // Headers complete
        let _ = parser.next_event().unwrap();

        // First chunk
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::Data(d) => assert_eq!(d, b"hello"),
            _ => panic!("Expected data"),
        }

        // Second chunk
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::Data(d) => assert_eq!(d, b" world"),
            _ => panic!("Expected data"),
        }

        // Complete
        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::Complete => {}
            _ => panic!("Expected complete"),
        }
    }

    #[test]
    fn test_parse_incremental() {
        let mut parser = ResponseParser::new();

        // Feed one byte at a time
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
        for byte in response.iter() {
            parser.feed(&[*byte]);
        }

        // Process all events until complete
        let mut event_count = 0;
        while !parser.is_complete() && event_count < 10 {
            match parser.next_event().unwrap() {
                Some(_) => event_count += 1,
                None => break,
            }
        }

        // Verify final state
        assert!(parser.is_complete());
    }

    #[test]
    fn test_204_no_content() {
        let mut parser = ResponseParser::new();
        parser.feed(b"HTTP/1.1 204 No Content\r\n\r\n");

        let _ = parser.next_event().unwrap();

        match parser.next_event().unwrap().unwrap() {
            ResponseEvent::HeadersComplete => {
                assert!(parser.is_complete());
            }
            _ => panic!("Expected headers complete and done"),
        }
    }

    #[test]
    fn test_case_insensitive_header_lookup() {
        let mut parser = ResponseParser::new();
        parser.feed(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n");

        let _ = parser.next_event().unwrap();
        let _ = parser.next_event().unwrap();
        let _ = parser.next_event().unwrap();

        assert_eq!(parser.get_header("Content-Type"), Some("text/html"));
        assert_eq!(parser.get_header("content-type"), Some("text/html"));
        assert_eq!(parser.get_header("CONTENT-TYPE"), Some("text/html"));
    }

    #[test]
    fn test_malformed_status_line() {
        let mut parser = ResponseParser::new();
        parser.feed(b"NOTAVALIDSTATUS\r\n\r\n");

        let result = parser.next_event();
        assert!(result.is_err() || (result.is_ok() && result.unwrap().is_none()));
    }

    #[test]
    fn test_malformed_header() {
        let mut parser = ResponseParser::new();
        parser.feed(b"HTTP/1.1 200 OK\r\nBadHeaderWithoutColon\r\n\r\n");

        let _ = parser.next_event().unwrap();  // Status
        while let Ok(Some(_)) = parser.next_event() {
            // Process events
        }
        // Should not panic
    }

    #[test]
    fn test_malformed_chunk_size() {
        let mut parser = ResponseParser::new();
        parser.feed(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n");
        parser.feed(b"BADSIZE\r\nhello\r\n");

        let _ = parser.next_event().unwrap();
        let _ = parser.next_event().unwrap();
        let _ = parser.next_event().unwrap();

        let result = parser.next_event();
        assert!(result.is_err());
    }

    #[test]
    fn test_oversized_header_block() {
        let mut parser = ResponseParser::new();
        let huge_header = vec![b'X'; 100000];
        parser.feed(b"HTTP/1.1 200 OK\r\n");
        parser.feed(&huge_header);

        let _ = parser.next_event().unwrap();

        let result = parser.next_event();
        assert!(result.is_err());
    }

    #[test]
    fn test_response_fixtures_with_splits() {
        // Test that responses parse the same whether fed all at once or incrementally
        let fixtures: Vec<&[u8]> = vec![
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
            b"HTTP/1.1 204 No Content\r\n\r\n",
        ];

        for fixture in fixtures {
            // Feed all at once
            let mut parser1 = ResponseParser::new();
            parser1.feed(fixture);
            let mut status1 = None;
            let mut complete1 = false;
            while let Ok(Some(event)) = parser1.next_event() {
                match event {
                    ResponseEvent::Status(s) => status1 = Some(s.code),
                    ResponseEvent::Complete => complete1 = true,
                    _ => {}
                }
            }

            // Feed in multiple chunks
            let mut parser2 = ResponseParser::new();
            let mid = fixture.len() / 2;
            parser2.feed(&fixture[..mid]);
            parser2.feed(&fixture[mid..]);
            let mut status2 = None;
            let mut complete2 = false;
            while let Ok(Some(event)) = parser2.next_event() {
                match event {
                    ResponseEvent::Status(s) => status2 = Some(s.code),
                    ResponseEvent::Complete => complete2 = true,
                    _ => {}
                }
            }

            assert_eq!(status1, status2, "Status mismatch for fixture");
            assert_eq!(complete1, complete2, "Complete mismatch for fixture");
        }
    }
}
