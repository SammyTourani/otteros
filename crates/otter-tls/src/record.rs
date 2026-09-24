//! The TLS record layer (RFC 8446 section 5): content types, the fixed
//! `legacy_record_version`, size limits, and `TLSInnerPlaintext`'s
//! content-type-then-zero-padding suffix. Actual encryption/decryption
//! lives in [`crate::key_schedule::TrafficKeys`] (it needs the active AEAD
//! and sequence number); this module is the plain framing rules shared by
//! both directions.

use alloc::vec::Vec;

/// `ContentType` (RFC 8446 section 5.1).
pub(crate) mod content_type {
    pub(crate) const CHANGE_CIPHER_SPEC: u8 = 20;
    pub(crate) const ALERT: u8 = 21;
    pub(crate) const HANDSHAKE: u8 = 22;
    pub(crate) const APPLICATION_DATA: u8 = 23;
}

/// `legacy_record_version`: fixed at `{3, 3}` for every record this client
/// sends (RFC 8446 section 5.1 permits `{3, 1}` for the very first
/// `ClientHello` only, as a compatibility nicety this client does not need).
pub(crate) const RECORD_VERSION: [u8; 2] = [0x03, 0x03];

/// The largest `TLSPlaintext.length` (RFC 8446 section 5.1): 2^14.
pub(crate) const MAX_PLAINTEXT_LEN: usize = 16384;

/// The largest `TLSCiphertext.length` (RFC 8446 section 5.2): 2^14 + 256.
pub(crate) const MAX_CIPHERTEXT_LEN: usize = 16384 + 256;

/// A record header: `ContentType`, `legacy_record_version`, `u16 length`.
pub(crate) type RecordHeader = [u8; 5];

pub(crate) fn build_header(content_type: u8, len: usize) -> RecordHeader {
    let len_bytes = (len as u16).to_be_bytes();
    [content_type, RECORD_VERSION[0], RECORD_VERSION[1], len_bytes[0], len_bytes[1]]
}

pub(crate) fn write_plaintext_record(out: &mut Vec<u8>, content_type: u8, payload: &[u8]) {
    out.extend_from_slice(&build_header(content_type, payload.len()));
    out.extend_from_slice(payload);
}

/// Strips `TLSInnerPlaintext`'s zero padding and trailing real content-type
/// byte (RFC 8446 section 5.2): scans from the end for the last non-zero
/// byte. Returns `None` if `buf` is all zero (no content type at all), which
/// section 5.4 says must be treated as `unexpected_message`.
pub(crate) fn strip_inner_plaintext(mut buf: Vec<u8>) -> Option<(u8, Vec<u8>)> {
    while buf.last() == Some(&0) {
        buf.pop();
    }
    let content_type = buf.pop()?;
    Some((content_type, buf))
}
