use alloc::string::{String, ToString};
use alloc::format;

/// URL parsing error
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// Missing scheme
    MissingScheme,
    /// Invalid scheme (only http/https allowed)
    InvalidScheme,
    /// Missing host
    MissingHost,
    /// Invalid IPv6 address
    InvalidIpv6,
    /// Invalid port
    InvalidPort,
}

/// Parsed URL
#[derive(Debug, Clone)]
pub struct Url {
    /// Scheme: "http" or "https"
    pub scheme: Scheme,
    /// Host (domain name, IPv4, or IPv6)
    pub host: String,
    /// Port number
    pub port: u16,
    /// Path (including leading /)
    pub path: String,
    /// Query string (not including ?)
    pub query: String,
}

/// URL scheme
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// HTTP
    Http,
    /// HTTPS
    Https,
}

impl Scheme {
    /// Get the default port for this scheme
    pub fn default_port(&self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }

    /// Get the scheme as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

impl Url {
    /// Parse a URL string according to RFC 3986 (subset)
    pub fn parse(url: &str) -> Result<Self, UrlError> {
        let url = url.trim();

        // Find scheme
        let (scheme_str, rest) = if let Some(pos) = url.find("://") {
            (&url[..pos], &url[pos + 3..])
        } else {
            return Err(UrlError::MissingScheme);
        };

        let scheme = match scheme_str {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            _ => return Err(UrlError::InvalidScheme),
        };

        // Find path/query (starts with / or ?)
        let (host_port, path_query) = if let Some(pos) = rest.find('/') {
            (&rest[..pos], &rest[pos..])
        } else if let Some(pos) = rest.find('?') {
            (&rest[..pos], &rest[pos..])
        } else {
            (rest, "/")
        };

        if host_port.is_empty() {
            return Err(UrlError::MissingHost);
        }

        // Parse host and port
        let (host, port) = parse_host_port(host_port, scheme)?;

        // Parse path and query
        let (path, query) = parse_path_query(path_query);

        Ok(Url {
            scheme,
            host,
            port,
            path,
            query,
        })
    }

    /// Resolve a relative reference against this URL (RFC 3986 section 5.2)
    pub fn resolve(&self, reference: &str) -> Result<Url, UrlError> {
        let reference = reference.trim();

        // Empty reference resolves to the same URL
        if reference.is_empty() {
            return Ok(Url {
                scheme: self.scheme,
                host: self.host.clone(),
                port: self.port,
                path: self.path.clone(),
                query: self.query.clone(),
            });
        }

        // If reference starts with scheme, parse it as absolute
        if reference.contains("://") {
            return Url::parse(reference);
        }

        // If reference starts with //, use same scheme but new host
        if reference.starts_with("//") {
            let new_url = format!("{}:{}", self.scheme.as_str(), reference);
            return Url::parse(&new_url);
        }

        // If reference starts with /, replace path
        if reference.starts_with('/') {
            let (path, query) = parse_path_query(reference);
            return Ok(Url {
                scheme: self.scheme,
                host: self.host.clone(),
                port: self.port,
                path,
                query,
            });
        }

        // If reference starts with ?, replace query only
        if let Some(query_part) = reference.strip_prefix('?') {
            let query = if let Some(pos) = query_part.find('#') {
                query_part[..pos].to_string()
            } else {
                query_part.to_string()
            };
            return Ok(Url {
                scheme: self.scheme,
                host: self.host.clone(),
                port: self.port,
                path: self.path.clone(),
                query,
            });
        }

        // Relative path reference
        let mut path = self.path.clone();

        // First, remove everything after the last / to get the directory
        if !path.ends_with('/') {
            if let Some(pos) = path.rfind('/') {
                path.truncate(pos + 1);
            } else {
                path = String::from("/");
            }
        }

        if let Some(rel_path) = reference.strip_prefix("./") {
            path.push_str(rel_path);
        } else if reference.starts_with("../") || reference == ".." || reference == "." {
            // Handle .. and . references
            let mut remaining = reference;

            while remaining.starts_with("../") || remaining == ".." {
                if remaining.starts_with("../") {
                    remaining = &remaining[3..];
                } else {
                    remaining = "";
                }

                // Remove the last directory
                if path.len() > 1 && path.ends_with('/') {
                    path.pop(); // Remove trailing /
                }
                if let Some(pos) = path.rfind('/') {
                    path.truncate(pos + 1);
                } else {
                    path = String::from("/");
                }
            }

            // Handle . reference
            if reference == "." {
                // Stays at current directory
            } else if !remaining.is_empty() {
                path.push_str(remaining);
            }
        } else {
            path.push_str(reference);
        }

        // Remove fragment
        if let Some(pos) = path.find('#') {
            path.truncate(pos);
        }

        // Extract query from path if present
        let (path, query) = if let Some(pos) = path.find('?') {
            (path[..pos].to_string(), path[pos + 1..].to_string())
        } else {
            (path, String::new())
        };

        Ok(Url {
            scheme: self.scheme,
            host: self.host.clone(),
            port: self.port,
            path,
            query,
        })
    }


    /// Convert to a normalized string representation
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let mut result = alloc::format!("{}://{}",self.scheme.as_str(), self.host);

        // Add port if not default
        if self.port != self.scheme.default_port() {
            result.push_str(&alloc::format!(":{}", self.port));
        }

        result.push_str(&self.path);

        if !self.query.is_empty() {
            result.push('?');
            result.push_str(&self.query);
        }

        result
    }
}

/// Parse host:port, handling IPv6 in brackets
fn parse_host_port(host_port: &str, scheme: Scheme) -> Result<(String, u16), UrlError> {
    // Handle IPv6 addresses in brackets
    if host_port.starts_with('[') {
        if let Some(end) = host_port.find(']') {
            let ipv6 = &host_port[1..end];

            // Validate IPv6 (basic check)
            if !is_valid_ipv6(ipv6) {
                return Err(UrlError::InvalidIpv6);
            }

            let port_part = &host_port[end + 1..];
            let port = if port_part.is_empty() {
                scheme.default_port()
            } else if let Some(port_str) = port_part.strip_prefix(':') {
                port_str.parse::<u16>()
                    .map_err(|_| UrlError::InvalidPort)?
            } else {
                return Err(UrlError::InvalidPort);
            };

            return Ok((ipv6.to_string(), port));
        } else {
            return Err(UrlError::InvalidIpv6);
        }
    }

    // Handle regular hosts and IPv4
    if let Some(colon_pos) = host_port.rfind(':') {
        let host = &host_port[..colon_pos];
        let port_str = &host_port[colon_pos + 1..];

        let port = port_str.parse::<u16>()
            .map_err(|_| UrlError::InvalidPort)?;

        Ok((host.to_string(), port))
    } else {
        Ok((host_port.to_string(), scheme.default_port()))
    }
}

/// Basic IPv6 validation (just check it's not empty and contains valid hex)
fn is_valid_ipv6(addr: &str) -> bool {
    if addr.is_empty() || addr.len() > 39 {
        return false;
    }

    // Allow hex digits, colons, and dots (for IPv4-mapped)
    addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
}

/// Parse path and query from path_query string (starts with / or ?)
fn parse_path_query(path_query: &str) -> (String, String) {
    if path_query.is_empty() {
        return (String::from("/"), String::new());
    }

    // Find the ? separator
    if let Some(pos) = path_query.find('?') {
        let path = path_query[..pos].to_string();
        let query = path_query[pos + 1..].to_string();
        let query = query.split('#').next().unwrap_or("").to_string();

        let path = if path.is_empty() { String::from("/") } else { path };
        (path, query)
    } else {
        // Just path, no query
        let path = path_query.split('#').next().unwrap_or(path_query);
        let path = if path.is_empty() { String::from("/") } else { path.to_string() };
        (path, String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_urls() {
        let url = Url::parse("http://example.com/path").unwrap();
        assert_eq!(url.scheme, Scheme::Http);
        assert_eq!(url.host, "example.com");
        assert_eq!(url.port, 80);
        assert_eq!(url.path, "/path");

        let url = Url::parse("https://example.com").unwrap();
        assert_eq!(url.scheme, Scheme::Https);
        assert_eq!(url.port, 443);
        assert_eq!(url.path, "/");
    }

    #[test]
    fn test_url_with_explicit_port() {
        let url = Url::parse("http://example.com:8080/path").unwrap();
        assert_eq!(url.port, 8080);
    }

    #[test]
    fn test_url_with_query() {
        let url = Url::parse("http://example.com/path?query=value").unwrap();
        assert_eq!(url.path, "/path");
        assert_eq!(url.query, "query=value");
    }

    #[test]
    fn test_url_with_fragment() {
        let url = Url::parse("http://example.com/path#fragment").unwrap();
        assert_eq!(url.path, "/path");
    }

    #[test]
    fn test_ipv4() {
        let url = Url::parse("http://192.168.1.1/").unwrap();
        assert_eq!(url.host, "192.168.1.1");
    }

    #[test]
    fn test_ipv6() {
        let url = Url::parse("http://[::1]/path").unwrap();
        assert_eq!(url.host, "::1");
        assert_eq!(url.port, 80);

        let url = Url::parse("http://[2001:db8::1]:8080/").unwrap();
        assert_eq!(url.host, "2001:db8::1");
        assert_eq!(url.port, 8080);
    }

    #[test]
    fn test_resolve_absolute_path() {
        let base = Url::parse("http://example.com/dir/file.html").unwrap();
        let resolved = base.resolve("/other").unwrap();
        assert_eq!(resolved.path, "/other");
        assert_eq!(resolved.host, "example.com");
    }

    #[test]
    fn test_resolve_relative_path() {
        let base = Url::parse("http://example.com/dir/file.html").unwrap();
        let resolved = base.resolve("other.html").unwrap();
        assert_eq!(resolved.path, "/dir/other.html");
    }

    #[test]
    fn test_resolve_parent_dir() {
        let base = Url::parse("http://example.com/dir/subdir/file.html").unwrap();
        let resolved = base.resolve("../other.html").unwrap();
        assert_eq!(resolved.path, "/dir/other.html");
    }

    #[test]
    fn test_resolve_absolute_url() {
        let base = Url::parse("http://example.com/").unwrap();
        let resolved = base.resolve("https://other.com/path").unwrap();
        assert_eq!(resolved.scheme, Scheme::Https);
        assert_eq!(resolved.host, "other.com");
    }

    #[test]
    fn test_scheme_errors() {
        assert!(Url::parse("ftp://example.com/").is_err());
        assert!(Url::parse("example.com/").is_err());
    }

    #[test]
    fn test_invalid_port() {
        assert!(Url::parse("http://example.com:invalid/").is_err());
    }

    #[test]
    fn test_ipv6_brackets_required() {
        // IPv6 must be in brackets
        let url = Url::parse("http://[::1]/").unwrap();
        assert_eq!(url.host, "::1");
    }

    #[test]
    fn test_url_to_string() {
        let url = Url::parse("http://example.com/path?query=value").unwrap();
        let s = url.to_string();
        assert!(s.contains("example.com"));
        assert!(s.contains("/path"));
        assert!(s.contains("query=value"));
    }

    #[test]
    fn test_url_rfc3986_normal_examples() {
        // RFC 3986 section 5.4.1 normal examples (core path resolution, no normalization)
        let base = Url::parse("http://a/b/c/d;p?q").unwrap();

        let cases = vec![
            ("g", "/b/c/g"),
            ("g/", "/b/c/g/"),
            ("/g", "/g"),
            ("g?y", "/b/c/g"),
            ("g#s", "/b/c/g"),
            (";x", "/b/c/;x"),
            ("g;x", "/b/c/g;x"),
            ("", "/b/c/d;p"),
            ("..", "/b/"),
            ("../", "/b/"),
            ("../g", "/b/g"),
            ("../../g", "/g"),
        ];

        for (reference, expected_path) in cases {
            match base.resolve(reference) {
                Ok(resolved) => {
                    assert_eq!(resolved.path, expected_path, "resolve('{}') path mismatch", reference);
                }
                Err(e) => {
                    panic!("Failed to resolve '{}': {:?}", reference, e);
                }
            }
        }
    }

    #[test]
    fn test_url_rfc3986_abnormal_examples() {
        // RFC 3986 section 5.4.2 abnormal examples (basic relative resolution only)
        let base = Url::parse("http://a/b/c/d;p?q").unwrap();

        let cases = vec![
            ("../../../g", "/g"),
            ("../../../../g", "/g"),
            ("g.", "/b/c/g."),
            ("..g", "/b/c/..g"),
        ];

        for (reference, expected_path) in cases {
            let resolved = base.resolve(reference).unwrap_or_else(|e| {
                panic!("resolve('{}') failed: {:?}", reference, e)
            });
            assert_eq!(resolved.path, expected_path, "resolve('{}') path mismatch", reference);
        }
    }

    #[test]
    fn test_url_parse_comprehensive() {
        // Test 30+ different URL parse cases
        let cases = vec![
            ("http://example.com", "example.com", 80, "/"),
            ("https://example.com", "example.com", 443, "/"),
            ("http://example.com/", "example.com", 80, "/"),
            ("http://example.com/path", "example.com", 80, "/path"),
            ("http://example.com/path/to/file", "example.com", 80, "/path/to/file"),
            ("http://example.com:8080", "example.com", 8080, "/"),
            ("http://example.com:8080/path", "example.com", 8080, "/path"),
            ("http://example.com/path?query", "example.com", 80, "/path"),
            ("http://example.com/path?q=v", "example.com", 80, "/path"),
            ("http://example.com/path#fragment", "example.com", 80, "/path"),
            ("https://example.com:443/path", "example.com", 443, "/path"),
            ("http://192.168.1.1", "192.168.1.1", 80, "/"),
            ("http://192.168.1.1:8080", "192.168.1.1", 8080, "/"),
            ("http://[::1]", "::1", 80, "/"),
            ("http://[::1]:8080", "::1", 8080, "/"),
            ("http://[2001:db8::1]", "2001:db8::1", 80, "/"),
            ("http://a.b.c.example.com", "a.b.c.example.com", 80, "/"),
            ("http://example.com/", "example.com", 80, "/"),
            ("http://example.com/a", "example.com", 80, "/a"),
            ("http://example.com/a/b", "example.com", 80, "/a/b"),
            ("http://example.com/a/b/c", "example.com", 80, "/a/b/c"),
            ("http://example.com/a/b/c/d", "example.com", 80, "/a/b/c/d"),
            ("http://example.com/;params", "example.com", 80, "/;params"),
            ("http://example.com/path;params", "example.com", 80, "/path;params"),
            ("http://example.com/path?a=b&c=d", "example.com", 80, "/path"),
            ("http://example.com/path?a=b#frag", "example.com", 80, "/path"),
            ("http://localhost/", "localhost", 80, "/"),
            ("http://localhost:3000/", "localhost", 3000, "/"),
            ("http://localhost:3000/api/v1", "localhost", 3000, "/api/v1"),
            ("http://example.test/", "example.test", 80, "/"),
        ];

        for (url_str, expected_host, expected_port, expected_path) in cases {
            match Url::parse(url_str) {
                Ok(url) => {
                    assert_eq!(url.host, expected_host, "host mismatch for {}", url_str);
                    assert_eq!(url.port, expected_port, "port mismatch for {}", url_str);
                    assert_eq!(url.path, expected_path, "path mismatch for {}", url_str);
                }
                Err(e) => {
                    panic!("Failed to parse '{}': {:?}", url_str, e);
                }
            }
        }
    }
}
