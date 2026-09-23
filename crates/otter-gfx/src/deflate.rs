//! A minimal zlib/DEFLATE *encoder*, used only to produce PNG screenshots. Only
//! stored and fixed-Huffman blocks are implemented (no LZ77 match search) --
//! correctness and simplicity matter far more than ratio for a debug screenshot.

use alloc::vec;
use alloc::vec::Vec;

use crate::checksum::adler32;

const MAX_BITS: usize = 15;

struct BitWriter {
    out: Vec<u8>,
    cur: u8,
    nbits: u32,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { out: Vec::new(), cur: 0, nbits: 0 }
    }

    fn write_bit(&mut self, bit: u32) {
        self.cur |= ((bit & 1) as u8) << self.nbits;
        self.nbits += 1;
        if self.nbits == 8 {
            self.out.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    /// Writes `n` bits of `value`, LSB-first (every DEFLATE field except Huffman
    /// codes themselves).
    fn write_bits_lsb(&mut self, value: u32, n: u32) {
        for i in 0..n {
            self.write_bit((value >> i) & 1);
        }
    }

    /// Writes a Huffman code MSB-first (DEFLATE's convention for codes).
    fn write_huffman_code(&mut self, code: u16, len: u8) {
        for i in (0..len).rev() {
            self.write_bit(((code >> i) & 1) as u32);
        }
    }

    fn align_to_byte(&mut self) {
        while self.nbits != 0 {
            self.write_bit(0);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.align_to_byte();
        self.out
    }
}

/// Canonical Huffman code assignment (RFC 1951 3.2.2) from per-symbol lengths.
fn canonical_codes(lengths: &[u8]) -> Vec<u16> {
    let mut counts = [0u32; MAX_BITS + 1];
    for &l in lengths {
        counts[l as usize] += 1;
    }
    counts[0] = 0;
    let mut code = 0u32;
    let mut next_code = [0u32; MAX_BITS + 1];
    for bits in 1..=MAX_BITS {
        code = (code + counts[bits - 1]) << 1;
        next_code[bits] = code;
    }
    let mut codes = vec![0u16; lengths.len()];
    for (sym, &l) in lengths.iter().enumerate() {
        if l != 0 {
            codes[sym] = next_code[l as usize] as u16;
            next_code[l as usize] += 1;
        }
    }
    codes
}

fn fixed_lengths() -> [u8; 288] {
    let mut lengths = [0u8; 288];
    lengths[0..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..288].fill(8);
    lengths
}

/// Encodes `data` as a series of raw-DEFLATE stored blocks (max 65535 bytes each).
pub fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 5);
    if data.is_empty() {
        out.push(0x01); // BFINAL=1, BTYPE=00, byte-aligned already
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
        return out;
    }
    let mut chunks = data.chunks(65535).peekable();
    while let Some(chunk) = chunks.next() {
        let is_last = chunks.peek().is_none();
        out.push(if is_last { 0x01 } else { 0x00 });
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out
}

/// Encodes `data` as one raw-DEFLATE fixed-Huffman block (no back-references: pure
/// literal-by-literal coding, still fully valid DEFLATE).
pub fn deflate_fixed(data: &[u8]) -> Vec<u8> {
    let lengths = fixed_lengths();
    let codes = canonical_codes(&lengths);
    let mut bw = BitWriter::new();
    bw.write_bit(1); // BFINAL
    bw.write_bits_lsb(0b01, 2); // BTYPE = fixed Huffman
    for &byte in data {
        bw.write_huffman_code(codes[byte as usize], lengths[byte as usize]);
    }
    bw.write_huffman_code(codes[256], lengths[256]); // end-of-block
    bw.finish()
}

const ZLIB_CMF: u8 = 0x78; // CM=8 (deflate), CINFO=7 (32K window)
const ZLIB_FLG: u8 = 0x01; // FCHECK bits chosen so (CMF*256+FLG) % 31 == 0, FDICT=0

/// Wraps a raw DEFLATE body in a zlib (RFC 1950) header and Adler-32 trailer.
fn wrap_zlib(body: Vec<u8>, source: &[u8]) -> Vec<u8> {
    let mut out = vec![ZLIB_CMF, ZLIB_FLG];
    out.extend(body);
    out.extend_from_slice(&adler32(source).to_be_bytes());
    out
}

/// A full zlib stream (stored blocks): simple, always valid, larger output.
pub fn zlib_compress_stored(data: &[u8]) -> Vec<u8> {
    wrap_zlib(deflate_stored(data), data)
}

/// A full zlib stream (one fixed-Huffman block): still simple, usually smaller
/// than the stored form for real image data.
pub fn zlib_compress_fixed(data: &[u8]) -> Vec<u8> {
    wrap_zlib(deflate_fixed(data), data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inflate::{inflate_raw, inflate_zlib};

    #[test]
    fn stored_round_trips_through_our_own_inflate() {
        let data = b"the quick brown fox jumps over the lazy dog 0123456789";
        assert_eq!(inflate_raw(&deflate_stored(data)).unwrap(), data);
    }

    #[test]
    fn stored_round_trips_empty() {
        assert_eq!(inflate_raw(&deflate_stored(b"")).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn stored_round_trips_across_a_65535_byte_chunk_boundary() {
        let data: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(inflate_raw(&deflate_stored(&data)).unwrap(), data);
    }

    #[test]
    fn fixed_huffman_round_trips_through_our_own_inflate() {
        let data = b"aaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbcccccccccc";
        assert_eq!(inflate_raw(&deflate_fixed(data)).unwrap(), data);
    }

    #[test]
    fn fixed_huffman_round_trips_all_byte_values() {
        let data: Vec<u8> = (0..=255u8).collect();
        assert_eq!(inflate_raw(&deflate_fixed(&data)).unwrap(), data);
    }

    #[test]
    fn zlib_stored_round_trips() {
        let data = b"otteros gfx crate";
        assert_eq!(inflate_zlib(&zlib_compress_stored(data)).unwrap(), data);
    }

    #[test]
    fn zlib_fixed_round_trips() {
        let data = b"otteros gfx crate, drawn one glyph at a time";
        assert_eq!(inflate_zlib(&zlib_compress_fixed(data)).unwrap(), data);
    }

    #[test]
    fn fixed_huffman_is_usually_smaller_than_stored_for_skewed_data() {
        let data = vec![0u8; 4096];
        assert!(deflate_fixed(&data).len() < deflate_stored(&data).len());
    }
}
