//! `otter-http`: OtterOS's HTTP/1.1 client library.
//!
//! A transport-agnostic HTTP/1.1 client that is used by `fetch`, the page
//! viewer and the Claude client (agent mode).
//!
//! DECISIONS.md D2 (no HTTP crates), D27 (pure no_std + alloc crate,
//! host-tested, zero external dependencies).
//!
//! ## Design
//!
//! 1. **URL parsing** (RFC 3986 subset): scheme http/https, host (DNS name,
//!    IPv4, bracketed IPv6), port with scheme defaults, path, query, fragment
//!    dropped from requests; percent-encoding helpers; relative reference
//!    resolution for redirects and page links.
//!
//! 2. **Sans-I/O client**: `Request { method, url, headers, body }` serialized
//!    with Host, User-Agent, Connection, Content-Length; a `ResponseParser`
//!    fed bytes incrementally that yields status line, headers (case-insensitive
//!    lookup, folded/duplicate handling), and the body via Content-Length,
//!    `Transfer-Encoding: chunked` (with trailers) or read-until-close; size
//!    limits on header block (64 KiB) and line length.
//!
//! 3. **Redirect handling helper** (301/302/303/307/308, max 5, method rules)
//!    for callers that loop.
//!
//! 4. **Server-sent events** (WHATWG event-stream format): incremental parser
//!    yielding events (event, data joined with newlines, id, retry), handling
//!    CR, LF and CRLF line endings and chunks split anywhere.
//!
//! 5. **Transport trait**: tiny blocking convenience layer so host tests can
//!    use `std::net::TcpStream` and, for HTTPS, `otter-tls` driven over that.

#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

extern crate alloc;

mod url;
mod request;
mod response;
mod redirect;
mod sse;

pub use url::{Url, UrlError};
pub use request::{Request, Method};
pub use response::{ResponseParser, Header, ResponseEvent, Status};
pub use redirect::{RedirectPolicy, should_redirect};
pub use sse::SseParser;

#[cfg(test)]
mod transport;

#[cfg(test)]
pub use transport::Transport;

/// Maximum header block size: 64 KiB
pub const MAX_HEADER_SIZE: usize = 64 * 1024;

/// Maximum single header line size
pub const MAX_LINE_SIZE: usize = 8192;

/// Error type for HTTP operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    /// URL parsing failed
    InvalidUrl,
    /// Invalid request
    InvalidRequest,
    /// Invalid response status line
    InvalidStatus,
    /// Invalid response header
    InvalidHeader,
    /// Header block size exceeded
    HeadersTooLarge,
    /// Header line too long
    LineTooLong,
    /// Invalid chunk size
    InvalidChunkSize,
    /// Unexpected end of input
    UnexpectedEof,
    /// Invalid Content-Length
    InvalidContentLength,
    /// Other HTTP error
    Other,
}
