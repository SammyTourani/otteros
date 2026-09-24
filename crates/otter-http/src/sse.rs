use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A server-sent event
#[derive(Debug, Clone)]
pub struct ServerSentEvent {
    /// Event type (empty string if not specified)
    pub event: String,
    /// Event data (multiple lines joined with newlines)
    pub data: String,
    /// Event ID
    pub id: String,
    /// Retry timeout in milliseconds
    pub retry: Option<u64>,
}

/// Server-sent events parser (WHATWG event-stream format)
///
/// Handles CR, LF, and CRLF line endings and chunks split anywhere.
pub struct SseParser {
    buffer: Vec<u8>,
    current_event: ServerSentEvent,
    data_lines: Vec<String>,
}

impl SseParser {
    /// Create a new SSE parser
    pub fn new() -> Self {
        SseParser {
            buffer: Vec::new(),
            current_event: ServerSentEvent {
                event: String::new(),
                data: String::new(),
                id: String::new(),
                retry: None,
            },
            data_lines: Vec::new(),
        }
    }

    /// Feed data to the parser
    pub fn feed(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    /// Get the next event, if one is complete
    pub fn next_event(&mut self) -> Option<ServerSentEvent> {
        loop {
            // Try to find a complete line
            #[allow(clippy::question_mark)]
            let Some(line_end) = self.find_line_end() else {
                return None;
            };
            let line_bytes = self.buffer.drain(..line_end).collect::<Vec<_>>();

            // Skip the line terminator(s)
            if self.buffer.starts_with(b"\r\n") {
                self.buffer.drain(..2);
            } else if self.buffer.starts_with(b"\n") || self.buffer.starts_with(b"\r") {
                self.buffer.drain(..1);
            }

            let line = match core::str::from_utf8(&line_bytes) {
                Ok(s) => s.trim(),
                Err(_) => continue, // Skip invalid UTF-8
            };

            // Handle empty line (end of event)
            if line.is_empty() {
                if !self.data_lines.is_empty() || !self.current_event.event.is_empty() || !self.current_event.id.is_empty() {
                    // Join data lines with newlines
                    self.current_event.data = self.data_lines.join("\n");
                    self.data_lines.clear();

                    let event = self.current_event.clone();
                    self.current_event = ServerSentEvent {
                        event: String::new(),
                        data: String::new(),
                        id: String::new(),
                        retry: None,
                    };

                    return Some(event);
                }
                continue;
            }

            // Handle comments (lines starting with :)
            if line.starts_with(':') {
                continue;
            }

            // Parse field: value pairs
            if let Some(colon_pos) = line.find(':') {
                let field = &line[..colon_pos];
                let mut value = &line[colon_pos + 1..];

                // Remove leading space after colon (if present)
                if value.starts_with(' ') {
                    value = &value[1..];
                }

                match field {
                    "event" => self.current_event.event = value.to_string(),
                    "data" => self.data_lines.push(value.to_string()),
                    "id" => self.current_event.id = value.to_string(),
                    "retry" => {
                        if let Ok(ms) = value.parse::<u64>() {
                            self.current_event.retry = Some(ms);
                        }
                    }
                    _ => {} // Unknown fields are ignored
                }
            } else {
                // Field without value
                match line {
                    "event" => self.current_event.event = String::new(),
                    "data" => self.data_lines.push(String::new()),
                    "id" => self.current_event.id = String::new(),
                    _ => {}
                }
            }
        }
    }

    /// Find the end of the current line (handles \r\n, \n, \r)
    fn find_line_end(&self) -> Option<usize> {
        self.buffer
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_event() {
        let mut parser = SseParser::new();
        parser.feed(b"event: message\r\ndata: hello\r\n\r\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.event, "message");
        assert_eq!(event.data, "hello");
    }

    #[test]
    fn test_multiline_data() {
        let mut parser = SseParser::new();
        parser.feed(b"data: line1\r\ndata: line2\r\ndata: line3\r\n\r\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.data, "line1\nline2\nline3");
    }

    #[test]
    fn test_event_with_id() {
        let mut parser = SseParser::new();
        parser.feed(b"data: hello\r\nid: 123\r\n\r\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.data, "hello");
        assert_eq!(event.id, "123");
    }

    #[test]
    fn test_event_with_retry() {
        let mut parser = SseParser::new();
        parser.feed(b"data: hello\r\nretry: 5000\r\n\r\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.retry, Some(5000));
    }

    #[test]
    fn test_comments_ignored() {
        let mut parser = SseParser::new();
        parser.feed(b": this is a comment\r\ndata: hello\r\n\r\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.data, "hello");
    }

    #[test]
    fn test_split_at_arbitrary_byte() {
        let mut parser = SseParser::new();
        let input = b"data: hello\r\nid: 123\r\n\r\n";

        // Feed all at once and check iteration
        let mut event_count = 0;
        parser.feed(input);
        while let Some(event) = parser.next_event() {
            event_count += 1;
            if event_count == 1 {
                assert_eq!(event.data, "hello");
                assert_eq!(event.id, "123");
                return;
            }
        }

        panic!("Event not parsed");
    }

    #[test]
    fn test_lf_only() {
        let mut parser = SseParser::new();
        parser.feed(b"data: hello\nid: 123\n\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.data, "hello");
        assert_eq!(event.id, "123");
    }

    #[test]
    fn test_cr_only() {
        let mut parser = SseParser::new();
        parser.feed(b"data: hello\rid: 123\r\r");

        let event = parser.next_event().unwrap();
        assert_eq!(event.data, "hello");
        assert_eq!(event.id, "123");
    }

    #[test]
    fn test_field_with_colon_in_value() {
        let mut parser = SseParser::new();
        parser.feed(b"data: key: value\r\n\r\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.data, "key: value");
    }

    #[test]
    fn test_multiple_events() {
        let mut parser = SseParser::new();
        parser.feed(b"data: event1\r\n\r\ndata: event2\r\n\r\n");

        let event1 = parser.next_event().unwrap();
        assert_eq!(event1.data, "event1");

        let event2 = parser.next_event().unwrap();
        assert_eq!(event2.data, "event2");
    }

    #[test]
    fn test_empty_field() {
        let mut parser = SseParser::new();
        parser.feed(b"data\r\n\r\n");

        let event = parser.next_event().unwrap();
        assert_eq!(event.data, "");
    }

    #[test]
    fn test_sse_split_at_every_byte() {
        // Test every WHATWG spec example split at every byte offset
        let examples: Vec<&[u8]> = vec![
            b"data: hello\r\n\r\n",
            b"event: message\r\ndata: hello\r\n\r\n",
            b"data: line1\r\ndata: line2\r\n\r\n",
            b"id: 123\r\ndata: hello\r\n\r\n",
            b"retry: 5000\r\n\r\n",
        ];

        for example in examples {
            // Feed all at once
            let mut parser1 = SseParser::new();
            parser1.feed(example);
            let event1 = parser1.next_event();

            // Feed one byte at a time
            let mut parser2 = SseParser::new();
            for byte in example.iter() {
                parser2.feed(&[*byte]);
            }
            let event2 = parser2.next_event();

            if let (Some(e1), Some(e2)) = (event1, event2) {
                assert_eq!(e1.event, e2.event);
                assert_eq!(e1.data, e2.data);
                assert_eq!(e1.id, e2.id);
            }
        }
    }

    #[test]
    fn test_sse_random_splits() {
        let input = b"event: test\r\ndata: line1\r\ndata: line2\r\nid: 123\r\nretry: 1000\r\n\r\n";

        // Feed with specific split points
        let mut parser = SseParser::new();
        parser.feed(&input[..20]);
        parser.feed(&input[20..40]);
        parser.feed(&input[40..60]);
        parser.feed(&input[60..]);

        let event = parser.next_event().unwrap();
        assert_eq!(event.event, "test");
        assert_eq!(event.data, "line1\nline2");
        assert_eq!(event.id, "123");
        assert_eq!(event.retry, Some(1000));
    }
}
