//! Small, bounds-checked helpers for TLS's wire encoding: fixed-width
//! big-endian integers and length-prefixed ("opaque") byte strings (RFC 8446
//! section 3's presentation language). Every read returns
//! [`TlsError::Decode`] instead of panicking on truncated or oversized
//! input -- this crate parses attacker-controlled network bytes, so a
//! malformed message is always an ordinary `Err`, never a panic.

use alloc::string::ToString;
use alloc::vec::Vec;

use crate::error::TlsError;

fn decode_err(msg: &'static str) -> TlsError {
    TlsError::Decode(msg.to_string())
}

/// A cursor over a byte slice with bounds-checked reads, for parsing one
/// handshake message body (or one extension's content) at a time.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], TlsError> {
        if self.remaining() < n {
            return Err(decode_err("truncated: not enough bytes remaining"));
        }
        let r = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(r)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, TlsError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, TlsError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn u24(&mut self) -> Result<u32, TlsError> {
        let b = self.take(3)?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }

    /// Reads a `u8`-length-prefixed opaque byte string (`opaque foo<0..255>`).
    pub(crate) fn opaque8(&mut self) -> Result<&'a [u8], TlsError> {
        let len = self.u8()? as usize;
        self.take(len)
    }

    /// Reads a `u16`-length-prefixed opaque byte string (`opaque foo<0..2^16-1>`).
    pub(crate) fn opaque16(&mut self) -> Result<&'a [u8], TlsError> {
        let len = self.u16()? as usize;
        self.take(len)
    }

    /// Reads a `u24`-length-prefixed opaque byte string (`opaque foo<0..2^24-1>`),
    /// the `Certificate` message's own length prefix (RFC 8446 section 4.4.2).
    pub(crate) fn opaque24(&mut self) -> Result<&'a [u8], TlsError> {
        let len = self.u24()? as usize;
        self.take(len)
    }

    /// Reads a `u16`-length-prefixed opaque byte string whose content is
    /// itself parsed with `f`, as a fresh sub-[`Reader`] that must be
    /// exhausted exactly (RFC 8446 messages never have trailing padding
    /// inside a length-prefixed field).
    pub(crate) fn nested16<T>(&mut self, f: impl FnOnce(&mut Reader<'a>) -> Result<T, TlsError>) -> Result<T, TlsError> {
        let bytes = self.opaque16()?;
        let mut inner = Reader::new(bytes);
        let v = f(&mut inner)?;
        if !inner.is_empty() {
            return Err(decode_err("trailing bytes inside a length-prefixed field"));
        }
        Ok(v)
    }
}

pub(crate) fn write_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}

pub(crate) fn write_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn write_u24(out: &mut Vec<u8>, v: usize) {
    let b = (v as u32).to_be_bytes();
    out.extend_from_slice(&b[1..]);
}

/// Writes `bytes` as a `u8`-length-prefixed opaque string. Panics if
/// `bytes.len() > 255`: every caller in this crate passes a fixed-size or
/// otherwise pre-bounded value (a cookie, a single extension), never
/// unbounded network input, so this is a programmer-error guard, not a
/// decode path.
pub(crate) fn write_opaque8(out: &mut Vec<u8>, bytes: &[u8]) {
    assert!(bytes.len() <= 0xff, "opaque8 value too long");
    write_u8(out, bytes.len() as u8);
    out.extend_from_slice(bytes);
}

/// Writes `bytes` as a `u16`-length-prefixed opaque string. See
/// [`write_opaque8`]'s panic note; this crate's own messages never approach
/// the 64 KiB limit.
pub(crate) fn write_opaque16(out: &mut Vec<u8>, bytes: &[u8]) {
    assert!(bytes.len() <= 0xffff, "opaque16 value too long");
    write_u16(out, bytes.len() as u16);
    out.extend_from_slice(bytes);
}

/// Writes `bytes` as a `u24`-length-prefixed opaque string (test-only: this
/// client only ever *parses* a `Certificate` message's `u24`-prefixed
/// fields, see [`crate::certificate`], but its tests need to build one).
#[cfg(test)]
pub(crate) fn write_opaque24(out: &mut Vec<u8>, bytes: &[u8]) {
    assert!(bytes.len() <= 0xff_ffff, "opaque24 value too long");
    write_u24(out, bytes.len());
    out.extend_from_slice(bytes);
}

/// Writes a `u16` length prefix, runs `f` to append the content, then
/// patches the prefix with the content's actual length -- for
/// `Extension.extension_data` and other `opaque<0..2^16-1>` fields whose
/// length is not known until after encoding it.
pub(crate) fn write_len16_prefixed(out: &mut Vec<u8>, f: impl FnOnce(&mut Vec<u8>)) {
    let len_pos = out.len();
    write_u16(out, 0);
    f(out);
    let len = out.len() - len_pos - 2;
    assert!(len <= 0xffff, "length-prefixed field too long");
    out[len_pos..len_pos + 2].copy_from_slice(&(len as u16).to_be_bytes());
}

/// A handshake message: `type` (RFC 8446 section 4) and body. Used both when
/// building a message this client sends and when one is parsed off the
/// reassembled handshake byte stream ([`crate::conn`]).
pub(crate) struct HandshakeMessage<'a> {
    pub(crate) msg_type: u8,
    pub(crate) body: &'a [u8],
}

pub(crate) mod handshake_type {
    pub(crate) const CLIENT_HELLO: u8 = 1;
    pub(crate) const SERVER_HELLO: u8 = 2;
    pub(crate) const NEW_SESSION_TICKET: u8 = 4;
    pub(crate) const ENCRYPTED_EXTENSIONS: u8 = 8;
    pub(crate) const CERTIFICATE: u8 = 11;
    pub(crate) const CERTIFICATE_REQUEST: u8 = 13;
    pub(crate) const CERTIFICATE_VERIFY: u8 = 15;
    pub(crate) const FINISHED: u8 = 20;
    pub(crate) const KEY_UPDATE: u8 = 24;
    pub(crate) const MESSAGE_HASH: u8 = 254;
}

/// Tries to parse one complete handshake message (a 4-byte `type`+`u24
/// length` header, then that many body bytes) off the front of `buf`.
/// Returns `None` (not an error) when `buf` does not yet hold a complete
/// message -- the caller should feed more bytes and try again -- which is
/// exactly RFC 8446 section 5.1's "handshake messages MAY be fragmented
/// across several records" reassembly.
pub(crate) fn parse_handshake_message(buf: &[u8]) -> Option<(HandshakeMessage<'_>, usize)> {
    if buf.len() < 4 {
        return None;
    }
    let msg_type = buf[0];
    let len = u32::from_be_bytes([0, buf[1], buf[2], buf[3]]) as usize;
    if buf.len() < 4 + len {
        return None;
    }
    Some((HandshakeMessage { msg_type, body: &buf[4..4 + len] }, 4 + len))
}

/// Writes one handshake message's header+body into `out`.
pub(crate) fn write_handshake_message(out: &mut Vec<u8>, msg_type: u8, body: &[u8]) {
    write_u8(out, msg_type);
    write_u24(out, body.len());
    out.extend_from_slice(body);
}
