use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::{Url, HttpError};

/// HTTP request method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET
    Get,
    /// HEAD
    Head,
    /// POST
    Post,
    /// PUT
    Put,
    /// DELETE
    Delete,
    /// PATCH
    Patch,
}

impl Method {
    /// Get the method as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
        }
    }

    /// Whether this method should have a body
    pub fn has_body(&self) -> bool {
        matches!(self, Method::Post | Method::Put | Method::Patch)
    }
}

/// HTTP request
#[derive(Debug, Clone)]
pub struct Request {
    /// HTTP method
    pub method: Method,
    /// URL
    pub url: Url,
    /// Headers (name, value pairs)
    pub headers: Vec<(String, String)>,
    /// Body
    pub body: Vec<u8>,
}

impl Request {
    /// Create a new GET request
    pub fn get(url: Url) -> Self {
        Request {
            method: Method::Get,
            url,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// Create a new POST request
    pub fn post(url: Url, body: Vec<u8>) -> Self {
        Request {
            method: Method::Post,
            url,
            headers: Vec::new(),
            body,
        }
    }

    /// Add a header
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Serialize the request to HTTP/1.1 format
    pub fn serialize(&self) -> Result<Vec<u8>, HttpError> {
        let mut result = Vec::new();

        // Request line: "METHOD /path?query HTTP/1.1\r\n"
        result.extend_from_slice(self.method.as_str().as_bytes());
        result.extend_from_slice(b" ");
        result.extend_from_slice(self.url.path.as_bytes());
        if !self.url.query.is_empty() {
            result.extend_from_slice(b"?");
            result.extend_from_slice(self.url.query.as_bytes());
        }
        result.extend_from_slice(b" HTTP/1.1\r\n");

        // Host header (required)
        result.extend_from_slice(b"Host: ");
        result.extend_from_slice(self.url.host.as_bytes());
        if self.url.port != self.url.scheme.default_port() {
            result.extend_from_slice(b":");
            result.extend_from_slice(self.url.port.to_string().as_bytes());
        }
        result.extend_from_slice(b"\r\n");

        // User-Agent header
        result.extend_from_slice(b"User-Agent: OtterOS/0.1\r\n");

        // Connection header
        result.extend_from_slice(b"Connection: close\r\n");

        // Content-Length if there's a body
        if !self.body.is_empty() {
            result.extend_from_slice(b"Content-Length: ");
            result.extend_from_slice(self.body.len().to_string().as_bytes());
            result.extend_from_slice(b"\r\n");
        }

        // Custom headers
        for (name, value) in &self.headers {
            result.extend_from_slice(name.as_bytes());
            result.extend_from_slice(b": ");
            result.extend_from_slice(value.as_bytes());
            result.extend_from_slice(b"\r\n");
        }

        // End of headers
        result.extend_from_slice(b"\r\n");

        // Body
        if !self.body.is_empty() {
            result.extend_from_slice(&self.body);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_request() {
        let url = Url::parse("http://example.com/path").unwrap();
        let request = Request::get(url);
        let serialized = request.serialize().unwrap();
        let s = String::from_utf8_lossy(&serialized);

        assert!(s.contains("GET /path HTTP/1.1"));
        assert!(s.contains("Host: example.com"));
        assert!(s.contains("User-Agent: OtterOS/0.1"));
        assert!(s.contains("Connection: close"));
    }

    #[test]
    fn test_request_with_query() {
        let url = Url::parse("http://example.com/path?query=value").unwrap();
        let request = Request::get(url);
        let serialized = request.serialize().unwrap();
        let s = String::from_utf8_lossy(&serialized);

        assert!(s.contains("GET /path?query=value HTTP/1.1"));
    }

    #[test]
    fn test_post_request() {
        let url = Url::parse("http://example.com/api").unwrap();
        let body = b"key=value".to_vec();
        let request = Request::post(url, body);
        let serialized = request.serialize().unwrap();
        let s = String::from_utf8_lossy(&serialized);

        assert!(s.contains("POST /api HTTP/1.1"));
        assert!(s.contains("Content-Length: 9"));
        assert!(s.contains("key=value"));
    }

    #[test]
    fn test_request_with_custom_headers() {
        let url = Url::parse("http://example.com/").unwrap();
        let request = Request::get(url)
            .with_header("Accept", "application/json")
            .with_header("X-Custom", "value");
        let serialized = request.serialize().unwrap();
        let s = String::from_utf8_lossy(&serialized);

        assert!(s.contains("Accept: application/json"));
        assert!(s.contains("X-Custom: value"));
    }

    #[test]
    fn test_request_with_explicit_port() {
        let url = Url::parse("http://example.com:8080/").unwrap();
        let request = Request::get(url);
        let serialized = request.serialize().unwrap();
        let s = String::from_utf8_lossy(&serialized);

        assert!(s.contains("Host: example.com:8080"));
    }

    #[test]
    fn test_https_default_port() {
        let url = Url::parse("https://example.com/").unwrap();
        let request = Request::get(url);
        let serialized = request.serialize().unwrap();
        let s = String::from_utf8_lossy(&serialized);

        // Default HTTPS port (443) should not be in Host header
        assert!(s.contains("Host: example.com\r\n"));
    }
}
