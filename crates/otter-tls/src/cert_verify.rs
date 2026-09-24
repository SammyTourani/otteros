//! `CertificateVerify` (RFC 8446 section 4.4.3): the exact byte string a
//! TLS 1.3 endpoint signs (64 spaces, a context string, a zero separator,
//! then the running transcript hash) and this message's own wire parsing.
//! [`crate::signature_scheme::verify`] does the actual signature check.

use alloc::vec::Vec;

use crate::codec::Reader;
use crate::error::TlsError;

/// RFC 8446 section 4.4.3's context string for a server's `CertificateVerify`
/// (a client never sends one -- this crate has no client certificates).
const SERVER_CONTEXT: &[u8] = b"TLS 1.3, server CertificateVerify";

/// Builds the exact byte string a server signs for `CertificateVerify`:
/// 64 `0x20` bytes, the context string, a `0x00` separator, then
/// `transcript_hash` (`Transcript-Hash(Handshake Context, Certificate)`,
/// i.e. up to and including the `Certificate` message but not this one).
pub(crate) fn server_signed_content(transcript_hash: &[u8]) -> Vec<u8> {
    let mut content = Vec::with_capacity(64 + SERVER_CONTEXT.len() + 1 + transcript_hash.len());
    content.extend(core::iter::repeat_n(0x20u8, 64));
    content.extend_from_slice(SERVER_CONTEXT);
    content.push(0x00);
    content.extend_from_slice(transcript_hash);
    content
}

/// Parses a `CertificateVerify` body: `struct { SignatureScheme algorithm;
/// opaque signature<0..2^16-1>; }`.
pub(crate) fn parse(body: &[u8]) -> Result<(u16, &[u8]), TlsError> {
    let mut r = Reader::new(body);
    let scheme = r.u16()?;
    let signature = r.opaque16()?;
    if !r.is_empty() {
        return Err(TlsError::Decode("CertificateVerify has trailing bytes".into()));
    }
    Ok((scheme, signature))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_content_has_the_rfc_8446_shape() {
        let hash = [0xabu8; 32];
        let content = server_signed_content(&hash);
        assert_eq!(&content[..64], &[0x20u8; 64][..]);
        assert_eq!(&content[64..64 + SERVER_CONTEXT.len()], SERVER_CONTEXT);
        assert_eq!(content[64 + SERVER_CONTEXT.len()], 0x00);
        assert_eq!(&content[64 + SERVER_CONTEXT.len() + 1..], &hash[..]);
    }

    #[test]
    fn parse_round_trips() {
        let mut body = Vec::new();
        body.extend_from_slice(&0x0403u16.to_be_bytes());
        body.extend_from_slice(&8u16.to_be_bytes());
        body.extend_from_slice(&[0u8; 8]);
        let (scheme, sig) = parse(&body).unwrap();
        assert_eq!(scheme, 0x0403);
        assert_eq!(sig, &[0u8; 8]);
    }
}
