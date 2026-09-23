//! PNG decode (colour types 0/2/3/4/6, bit depths 1-8 and 16, all five filters,
//! `tRNS`, CRC-32 verified) and a simple always-valid PNG encoder (RGBA8, `None`
//! filter, one fixed-Huffman zlib block) for screenshots.
//!
//! Adam7 interlacing is detected and rejected with [`PngError::Adam7NotSupported`]
//! rather than implemented (DESIGN.md's brief explicitly allows "supported or
//! rejected with a clear error").

use alloc::vec;
use alloc::vec::Vec;

use crate::checksum::Crc32;
use crate::color::Color;
use crate::deflate::zlib_compress_fixed;
use crate::inflate::{inflate_zlib, InflateError};
use crate::surface::Surface;

/// Everything that can go wrong decoding a PNG. Malformed input always produces
/// one of these, never a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngError {
    /// The 8-byte PNG signature didn't match.
    BadSignature,
    /// A chunk's declared length ran past the end of the file.
    Truncated,
    /// A chunk's CRC-32 didn't match its declared value.
    CrcMismatch,
    /// No `IHDR` chunk was found.
    MissingIhdr,
    /// `IHDR` used an unsupported compression/filter method, or a bad combination
    /// of colour type and bit depth.
    InvalidIhdr,
    /// Colour type 3 (palette) with no `PLTE` chunk.
    MissingPlte,
    /// A colour type outside 0/2/3/4/6.
    UnsupportedColorType,
    /// A row used a filter type byte outside 0..=4.
    InvalidFilterType,
    /// Width or height was zero.
    ZeroDimensions,
    /// `IHDR` declared Adam7 interlacing (method 1), which is not implemented.
    Adam7NotSupported,
    /// The concatenated `IDAT` data failed to decompress.
    Inflate(InflateError),
}

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

struct Ihdr {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
}

fn parse_ihdr(data: &[u8]) -> Result<Ihdr, PngError> {
    if data.len() < 13 {
        return Err(PngError::Truncated);
    }
    let width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let bit_depth = data[8];
    let color_type = data[9];
    let compression = data[10];
    let filter_method = data[11];
    let interlace = data[12];
    if compression != 0 || filter_method != 0 {
        return Err(PngError::InvalidIhdr);
    }
    Ok(Ihdr { width, height, bit_depth, color_type, interlace })
}

fn channels_for(color_type: u8) -> usize {
    match color_type {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => 0,
    }
}

fn validate_bit_depth(color_type: u8, bit_depth: u8) -> bool {
    match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16),
        2 | 4 | 6 => matches!(bit_depth, 8 | 16),
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),
        _ => false,
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn unfilter(raw: &[u8], width_bytes: usize, height: usize, bpp: usize) -> Result<Vec<u8>, PngError> {
    let row_stride = width_bytes + 1;
    if raw.len() < row_stride * height {
        return Err(PngError::Truncated);
    }
    let mut out = vec![0u8; width_bytes * height];
    let mut prev_row = vec![0u8; width_bytes];
    for y in 0..height {
        let row_start = y * row_stride;
        let filter_type = raw[row_start];
        let src = &raw[row_start + 1..row_start + 1 + width_bytes];
        let dst_start = y * width_bytes;
        for x in 0..width_bytes {
            let a = if x >= bpp { out[dst_start + x - bpp] } else { 0 };
            let b = prev_row[x];
            let c = if x >= bpp { prev_row[x - bpp] } else { 0 };
            let recon = match filter_type {
                0 => src[x],
                1 => src[x].wrapping_add(a),
                2 => src[x].wrapping_add(b),
                3 => src[x].wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => src[x].wrapping_add(paeth(a, b, c)),
                _ => return Err(PngError::InvalidFilterType),
            };
            out[dst_start + x] = recon;
        }
        prev_row.copy_from_slice(&out[dst_start..dst_start + width_bytes]);
    }
    Ok(out)
}

/// Unpacks `count` samples of `bit_depth` bits (MSB-first packing, PNG's
/// convention) from one already-defiltered scanline. Out-of-range reads (a
/// truncated row) yield `0` rather than panicking.
fn unpack_samples(row: &[u8], bit_depth: u8, count: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(count);
    let get = |i: usize| -> u8 { row.get(i).copied().unwrap_or(0) };
    match bit_depth {
        16 => {
            for i in 0..count {
                out.push(((get(i * 2) as u16) << 8) | get(i * 2 + 1) as u16);
            }
        }
        8 => {
            for i in 0..count {
                out.push(get(i) as u16);
            }
        }
        _ => {
            let mask = (1u16 << bit_depth) - 1;
            let mut bit_pos = 0usize;
            for _ in 0..count {
                let byte_idx = bit_pos / 8;
                let bit_off = bit_pos % 8;
                let shift = 8 - bit_depth as usize - bit_off;
                out.push((get(byte_idx) as u16 >> shift) & mask);
                bit_pos += bit_depth as usize;
            }
        }
    }
    out
}

fn expand_to_8(sample: u16, bit_depth: u8) -> u8 {
    match bit_depth {
        1 => if sample != 0 { 255 } else { 0 },
        2 => (sample * 85) as u8,
        4 => (sample * 17) as u8,
        16 => (sample >> 8) as u8,
        _ => sample as u8,
    }
}

fn decode_row(surface: &mut Surface, y: usize, row: &[u8], ihdr: &Ihdr, plte: Option<&[u8]>, trns: Option<&[u8]>) -> Result<(), PngError> {
    let width = ihdr.width as usize;
    let base = y * surface.stride;
    match ihdr.color_type {
        0 => {
            let samples = unpack_samples(row, ihdr.bit_depth, width);
            let trns_gray = trns.filter(|t| t.len() >= 2).map(|t| u16::from_be_bytes([t[0], t[1]]));
            for (x, &raw_v) in samples.iter().enumerate() {
                let v = expand_to_8(raw_v, ihdr.bit_depth);
                let a = if trns_gray == Some(raw_v) { 0 } else { 255 };
                surface.px[base + x] = Color::rgba(v, v, v, a).premul();
            }
        }
        2 => {
            let samples = unpack_samples(row, ihdr.bit_depth, width * 3);
            let trns_rgb = trns.filter(|t| t.len() >= 6).map(|t| {
                (u16::from_be_bytes([t[0], t[1]]), u16::from_be_bytes([t[2], t[3]]), u16::from_be_bytes([t[4], t[5]]))
            });
            for x in 0..width {
                let (rr, gr, br) = (samples[x * 3], samples[x * 3 + 1], samples[x * 3 + 2]);
                let a = if trns_rgb == Some((rr, gr, br)) { 0 } else { 255 };
                let (r, g, b) = (expand_to_8(rr, ihdr.bit_depth), expand_to_8(gr, ihdr.bit_depth), expand_to_8(br, ihdr.bit_depth));
                surface.px[base + x] = Color::rgba(r, g, b, a).premul();
            }
        }
        3 => {
            let plte = plte.ok_or(PngError::MissingPlte)?;
            let samples = unpack_samples(row, ihdr.bit_depth, width);
            for (x, &idx) in samples.iter().enumerate() {
                let idx = idx as usize;
                let (r, g, b) = if idx * 3 + 2 < plte.len() { (plte[idx * 3], plte[idx * 3 + 1], plte[idx * 3 + 2]) } else { (0, 0, 0) };
                let a = trns.and_then(|t| t.get(idx).copied()).unwrap_or(255);
                surface.px[base + x] = Color::rgba(r, g, b, a).premul();
            }
        }
        4 => {
            let samples = unpack_samples(row, ihdr.bit_depth, width * 2);
            for x in 0..width {
                let v = expand_to_8(samples[x * 2], ihdr.bit_depth);
                let a = expand_to_8(samples[x * 2 + 1], ihdr.bit_depth);
                surface.px[base + x] = Color::rgba(v, v, v, a).premul();
            }
        }
        6 => {
            let samples = unpack_samples(row, ihdr.bit_depth, width * 4);
            for x in 0..width {
                let r = expand_to_8(samples[x * 4], ihdr.bit_depth);
                let g = expand_to_8(samples[x * 4 + 1], ihdr.bit_depth);
                let b = expand_to_8(samples[x * 4 + 2], ihdr.bit_depth);
                let a = expand_to_8(samples[x * 4 + 3], ihdr.bit_depth);
                surface.px[base + x] = Color::rgba(r, g, b, a).premul();
            }
        }
        _ => return Err(PngError::UnsupportedColorType),
    }
    Ok(())
}

/// Decodes a PNG file's bytes into a premultiplied-ARGB [`Surface`].
pub fn decode(data: &[u8]) -> Result<Surface, PngError> {
    if data.len() < 8 || data[0..8] != SIGNATURE {
        return Err(PngError::BadSignature);
    }
    let mut pos = 8usize;
    let mut ihdr: Option<Ihdr> = None;
    let mut idat = Vec::new();
    let mut plte: Option<Vec<u8>> = None;
    let mut trns: Option<Vec<u8>> = None;
    let mut seen_iend = false;

    while pos + 8 <= data.len() && !seen_iend {
        let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let ctype = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
        let data_start = pos + 8;
        if data_start.checked_add(len).and_then(|e| e.checked_add(4)).is_none_or(|end| end > data.len()) {
            return Err(PngError::Truncated);
        }
        let chunk_data = &data[data_start..data_start + len];
        let crc_stored = u32::from_be_bytes(
            data[data_start + len..data_start + len + 4].try_into().expect("checked above"),
        );
        let mut crc = Crc32::new();
        crc.update(&ctype);
        crc.update(chunk_data);
        if crc.finish() != crc_stored {
            return Err(PngError::CrcMismatch);
        }

        match &ctype {
            b"IHDR" => ihdr = Some(parse_ihdr(chunk_data)?),
            b"PLTE" => plte = Some(chunk_data.to_vec()),
            b"tRNS" => trns = Some(chunk_data.to_vec()),
            b"IDAT" => idat.extend_from_slice(chunk_data),
            b"IEND" => seen_iend = true,
            _ => {}
        }
        pos = data_start + len + 4;
    }

    if !seen_iend {
        return Err(PngError::Truncated);
    }
    let ihdr = ihdr.ok_or(PngError::MissingIhdr)?;
    if ihdr.width == 0 || ihdr.height == 0 {
        return Err(PngError::ZeroDimensions);
    }
    if ihdr.interlace == 1 {
        return Err(PngError::Adam7NotSupported);
    }
    if ihdr.interlace != 0 {
        return Err(PngError::InvalidIhdr);
    }
    if !validate_bit_depth(ihdr.color_type, ihdr.bit_depth) {
        return Err(PngError::InvalidIhdr);
    }
    if ihdr.color_type == 3 && plte.is_none() {
        return Err(PngError::MissingPlte);
    }

    let raw = inflate_zlib(&idat).map_err(PngError::Inflate)?;
    let channels = channels_for(ihdr.color_type);
    let bits_per_pixel = channels * ihdr.bit_depth as usize;
    let row_bytes = bits_per_pixel.saturating_mul(ihdr.width as usize).div_ceil(8);
    let bpp = bits_per_pixel.div_ceil(8).max(1);
    let unfiltered = unfilter(&raw, row_bytes, ihdr.height as usize, bpp)?;

    let mut surface = Surface::new(ihdr.width as usize, ihdr.height as usize);
    for y in 0..ihdr.height as usize {
        let row = &unfiltered[y * row_bytes..(y + 1) * row_bytes];
        decode_row(&mut surface, y, row, &ihdr, plte.as_deref(), trns.as_deref())?;
    }
    Ok(surface)
}

fn write_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ctype);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(ctype);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// Encodes a [`Surface`] as an 8-bit RGBA PNG (`None` filter, one fixed-Huffman
/// zlib block): simple and always valid, which is all a debug screenshot needs.
pub fn encode(surface: &Surface) -> Vec<u8> {
    let mut raw = Vec::with_capacity(surface.h * (1 + surface.w * 4));
    for y in 0..surface.h {
        raw.push(0); // filter type: None
        for x in 0..surface.w {
            let c = Color::unpremul(surface.px[y * surface.stride + x]);
            raw.extend_from_slice(&[c.r, c.g, c.b, c.a]);
        }
    }
    let compressed = zlib_compress_fixed(&raw);

    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr_data = Vec::with_capacity(13);
    ihdr_data.extend_from_slice(&(surface.w as u32).to_be_bytes());
    ihdr_data.extend_from_slice(&(surface.h as u32).to_be_bytes());
    ihdr_data.extend_from_slice(&[8, 6, 0, 0, 0]); // depth 8, RGBA, compression/filter/interlace 0
    write_chunk(&mut out, b"IHDR", &ihdr_data);
    write_chunk(&mut out, b"IDAT", &compressed);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker_surface(w: usize, h: usize) -> Surface {
        let mut s = Surface::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let on = (x + y) % 2 == 0;
                let c = if on { Color::rgba(200, 50, 10, 255) } else { Color::rgba(10, 20, 220, 128) };
                s.px[y * s.stride + x] = c.premul();
            }
        }
        s
    }

    #[test]
    fn encode_decode_round_trip_opaque() {
        let mut s = Surface::new(4, 3);
        for (i, p) in s.px.iter_mut().enumerate() {
            *p = Color::rgb((i * 17) as u8, (i * 31) as u8, (i * 53) as u8).premul();
        }
        let bytes = encode(&s);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.w, 4);
        assert_eq!(decoded.h, 3);
        assert_eq!(decoded.px, s.px);
    }

    #[test]
    fn encode_decode_round_trip_with_alpha() {
        let s = checker_surface(5, 5);
        let decoded = decode(&encode(&s)).unwrap();
        assert_eq!(decoded.px, s.px);
    }

    #[test]
    fn rejects_bad_signature() {
        assert_eq!(decode(b"not a png"), Err(PngError::BadSignature));
    }

    #[test]
    fn rejects_truncated_file() {
        let s = Surface::new(2, 2);
        let bytes = encode(&s);
        assert!(matches!(decode(&bytes[..bytes.len() - 10]), Err(PngError::Truncated) | Err(PngError::Inflate(_))));
    }

    #[test]
    fn detects_crc_mismatch() {
        let s = Surface::new(2, 2);
        let mut bytes = encode(&s);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF; // corrupt IEND's CRC
        assert_eq!(decode(&bytes), Err(PngError::CrcMismatch));
    }

    #[test]
    fn rejects_missing_palette() {
        // Hand-build a minimal color-type-3 PNG with no PLTE chunk.
        let mut out = Vec::new();
        out.extend_from_slice(&SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 3, 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        let idat = zlib_compress_fixed(&[0, 0]);
        write_chunk(&mut out, b"IDAT", &idat);
        write_chunk(&mut out, b"IEND", &[]);
        assert_eq!(decode(&out), Err(PngError::MissingPlte));
    }

    #[test]
    fn rejects_zero_dimensions() {
        let mut out = Vec::new();
        out.extend_from_slice(&SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&0u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        write_chunk(&mut out, b"IHDR", &ihdr);
        write_chunk(&mut out, b"IEND", &[]);
        assert_eq!(decode(&out), Err(PngError::ZeroDimensions));
    }

    #[test]
    fn rejects_adam7_interlace() {
        let mut out = Vec::new();
        out.extend_from_slice(&SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 1]); // interlace = 1 (Adam7)
        write_chunk(&mut out, b"IHDR", &ihdr);
        write_chunk(&mut out, b"IEND", &[]);
        assert_eq!(decode(&out), Err(PngError::Adam7NotSupported));
    }

    #[test]
    fn paeth_matches_reference_cases() {
        assert_eq!(paeth(0, 0, 0), 0);
        assert_eq!(paeth(10, 20, 0), 20); // predicts b when c is far
        assert_eq!(paeth(10, 0, 0), 10);
    }

    #[test]
    fn unpack_samples_bit_depth_one() {
        // Byte 0b1010_0110 -> bits MSB-first: 1,0,1,0,0,1,1,0
        let row = [0b1010_0110u8];
        let samples = unpack_samples(&row, 1, 8);
        assert_eq!(samples, alloc::vec![1, 0, 1, 0, 0, 1, 1, 0]);
    }

    #[test]
    fn unpack_samples_bit_depth_four() {
        let row = [0xAB]; // top nibble 0xA, bottom 0xB
        let samples = unpack_samples(&row, 4, 2);
        assert_eq!(samples, alloc::vec![0xA, 0xB]);
    }

    #[test]
    fn expand_to_8_covers_every_depth() {
        assert_eq!(expand_to_8(1, 1), 255);
        assert_eq!(expand_to_8(0, 1), 0);
        assert_eq!(expand_to_8(3, 2), 255);
        assert_eq!(expand_to_8(15, 4), 255);
        assert_eq!(expand_to_8(255, 8), 255);
        assert_eq!(expand_to_8(0xFF00, 16), 255);
    }
}
