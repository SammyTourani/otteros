//! Bounds-checked big-endian byte reading, shared by every sfnt table parser.

use super::FontError;

/// A cursor-free, bounds-checked reader over an sfnt table's bytes. Every method
/// returns `Err(FontError::Truncated)` instead of panicking on out-of-range input,
/// which is what lets the font parser reject malformed/fuzzed fonts cleanly.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    pub data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn u8(&self, off: usize) -> Result<u8, FontError> {
        self.data.get(off).copied().ok_or(FontError::Truncated)
    }

    pub fn i8(&self, off: usize) -> Result<i8, FontError> {
        Ok(self.u8(off)? as i8)
    }

    pub fn u16(&self, off: usize) -> Result<u16, FontError> {
        let b = self.data.get(off..off + 2).ok_or(FontError::Truncated)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn i16(&self, off: usize) -> Result<i16, FontError> {
        Ok(self.u16(off)? as i16)
    }

    pub fn u32(&self, off: usize) -> Result<u32, FontError> {
        let b = self.data.get(off..off + 4).ok_or(FontError::Truncated)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// An `F2Dot14` fixed-point value (used by composite glyph transforms).
    pub fn f2dot14(&self, off: usize) -> Result<f32, FontError> {
        Ok(self.i16(off)? as f32 / 16384.0)
    }

    pub fn tag(&self, off: usize) -> Result<[u8; 4], FontError> {
        let b = self.data.get(off..off + 4).ok_or(FontError::Truncated)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    pub fn slice(&self, off: usize, len: usize) -> Result<&'a [u8], FontError> {
        self.data.get(off..off + len).ok_or(FontError::Truncated)
    }

    /// A sub-reader over `off..off+len`.
    pub fn sub(&self, off: usize, len: usize) -> Result<Reader<'a>, FontError> {
        Ok(Reader::new(self.slice(off, len)?))
    }
}
