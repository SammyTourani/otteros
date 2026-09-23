//! `cmap`: Unicode-to-glyph mapping, formats 4 (BMP, segmented) and 12
//! (full Unicode, segmented with 32-bit code points).

use super::tables::Reader;
use super::FontError;

/// A parsed reference to whichever `cmap` subtable was chosen as "best".
pub enum CmapSubtable<'a> {
    /// Format 4: BMP-only, delta/range-offset segments.
    Format4(Reader<'a>),
    /// Format 12: full Unicode, contiguous (start, end, startGlyph) groups.
    Format12(Reader<'a>),
}

fn score_subtable(platform_id: u16, encoding_id: u16) -> i32 {
    match (platform_id, encoding_id) {
        (3, 10) => 100, // Windows, UCS-4 (almost always format 12)
        (0, 4) | (0, 6) => 95, // Unicode, full repertoire
        (3, 1) => 80,   // Windows, BMP (almost always format 4)
        (0, 3) => 75,   // Unicode BMP
        (0, _) => 50,
        (3, 0) => 10, // symbol encoding, last resort
        _ => 0,
    }
}

/// Picks the best-scoring subtable this parser understands (format 4 or 12) out of
/// the `cmap` table's directory.
pub fn parse_best_subtable(cmap: Reader<'_>) -> Result<CmapSubtable<'_>, FontError> {
    let num_tables = cmap.u16(2)?;
    let mut best: Option<(i32, u32)> = None;
    for i in 0..num_tables {
        let rec = 4 + i as usize * 8;
        let platform_id = cmap.u16(rec)?;
        let encoding_id = cmap.u16(rec + 2)?;
        let offset = cmap.u32(rec + 4)?;
        let score = score_subtable(platform_id, encoding_id);
        if score > 0
            && best.is_none_or(|(s, _)| score > s)
            && let Ok(format) = cmap.u16(offset as usize)
            && (format == 4 || format == 12)
        {
            // Only keep this candidate if we can actually read its format.
            best = Some((score, offset));
        }
    }
    let (_, offset) = best.ok_or(FontError::NoUsableCmap)?;
    let format = cmap.u16(offset as usize)?;
    let rest = cmap.len() - offset as usize;
    match format {
        4 => Ok(CmapSubtable::Format4(cmap.sub(offset as usize, rest)?)),
        12 => Ok(CmapSubtable::Format12(cmap.sub(offset as usize, rest)?)),
        _ => Err(FontError::NoUsableCmap),
    }
}

fn lookup_format4(t: &Reader, c: u32) -> Option<u16> {
    if c > 0xFFFF {
        return None;
    }
    let c = c as u16;
    let seg_count = (t.u16(6).ok()? / 2) as usize;
    let end_codes = 14usize;
    let start_codes = end_codes + seg_count * 2 + 2; // +2: reservedPad
    let id_deltas = start_codes + seg_count * 2;
    let id_range_offsets = id_deltas + seg_count * 2;

    for seg in 0..seg_count {
        let end_code = t.u16(end_codes + seg * 2).ok()?;
        if c > end_code {
            continue;
        }
        let start_code = t.u16(start_codes + seg * 2).ok()?;
        if c < start_code {
            return Some(0);
        }
        let id_delta = t.i16(id_deltas + seg * 2).ok()?;
        let range_offset_addr = id_range_offsets + seg * 2;
        let id_range_offset = t.u16(range_offset_addr).ok()?;
        if id_range_offset == 0 {
            return Some((c as i32 + id_delta as i32) as u16);
        }
        let glyph_addr = range_offset_addr + id_range_offset as usize + (c - start_code) as usize * 2;
        let g = t.u16(glyph_addr).ok()?;
        if g == 0 {
            return Some(0);
        }
        return Some((g as i32 + id_delta as i32) as u16);
    }
    None
}

fn lookup_format12(t: &Reader, c: u32) -> Option<u16> {
    let num_groups = t.u32(12).ok()?;
    // Groups are sorted by startCharCode; binary search.
    let mut lo: i64 = 0;
    let mut hi: i64 = num_groups as i64 - 1;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let rec = 16 + mid as usize * 12;
        let start = t.u32(rec).ok()?;
        let end = t.u32(rec + 4).ok()?;
        if c < start {
            hi = mid - 1;
        } else if c > end {
            lo = mid + 1;
        } else {
            let start_glyph = t.u32(rec + 8).ok()?;
            return Some((start_glyph + (c - start)) as u16);
        }
    }
    None
}

/// Looks up a Unicode scalar value in the chosen subtable.
pub fn lookup(sub: &CmapSubtable<'_>, c: u32) -> Option<u16> {
    match sub {
        CmapSubtable::Format4(t) => lookup_format4(t, c),
        CmapSubtable::Format12(t) => lookup_format12(t, c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// Hand-builds a minimal format-4 subtable with one segment mapping
    /// `'A'..='C'` (0x41..=0x43) to glyphs `5..=7`, plus the mandatory trailing
    /// all-0xFFFF terminator segment.
    fn build_format4() -> Vec<u8> {
        let end_codes: [u16; 2] = [0x43, 0xFFFF];
        let start_codes: [u16; 2] = [0x41, 0xFFFF];
        let id_deltas: [i16; 2] = [5 - 0x41, 1]; // glyph = c + delta
        let id_range_offsets: [u16; 2] = [0, 0];
        let seg_count = 2u16;
        let mut out = Vec::new();
        out.extend_from_slice(&4u16.to_be_bytes()); // format
        out.extend_from_slice(&0u16.to_be_bytes()); // length (unused by our reader)
        out.extend_from_slice(&0u16.to_be_bytes()); // language
        out.extend_from_slice(&(seg_count * 2).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
        out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        for e in end_codes {
            out.extend_from_slice(&e.to_be_bytes());
        }
        out.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for s in start_codes {
            out.extend_from_slice(&s.to_be_bytes());
        }
        for d in id_deltas {
            out.extend_from_slice(&d.to_be_bytes());
        }
        for r in id_range_offsets {
            out.extend_from_slice(&r.to_be_bytes());
        }
        out
    }

    #[test]
    fn format4_maps_ascii_range() {
        let bytes = build_format4();
        let t = Reader::new(&bytes);
        assert_eq!(lookup_format4(&t, 'A' as u32), Some(5));
        assert_eq!(lookup_format4(&t, 'B' as u32), Some(6));
        assert_eq!(lookup_format4(&t, 'C' as u32), Some(7));
    }

    #[test]
    fn format4_returns_notdef_outside_mapped_range() {
        let bytes = build_format4();
        let t = Reader::new(&bytes);
        assert_eq!(lookup_format4(&t, 'Z' as u32), Some(0));
    }

    #[test]
    fn format4_rejects_above_bmp() {
        let bytes = build_format4();
        let t = Reader::new(&bytes);
        assert_eq!(lookup_format4(&t, 0x1F600), None);
    }

    fn build_format12(groups: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&12u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // reserved
        out.extend_from_slice(&0u32.to_be_bytes()); // length
        out.extend_from_slice(&0u32.to_be_bytes()); // language
        out.extend_from_slice(&(groups.len() as u32).to_be_bytes());
        for &(s, e, g) in groups {
            out.extend_from_slice(&s.to_be_bytes());
            out.extend_from_slice(&e.to_be_bytes());
            out.extend_from_slice(&g.to_be_bytes());
        }
        out
    }

    #[test]
    fn format12_maps_astral_and_bmp() {
        let bytes = build_format12(&[(0x41, 0x5A, 100), (0x1F600, 0x1F64F, 2000)]);
        let t = Reader::new(&bytes);
        assert_eq!(lookup_format12(&t, 'A' as u32), Some(100));
        assert_eq!(lookup_format12(&t, 'Z' as u32), Some(125));
        assert_eq!(lookup_format12(&t, 0x1F600), Some(2000));
        assert_eq!(lookup_format12(&t, 0x1F650), None);
    }

    #[test]
    fn format12_handles_e_acute() {
        let bytes = build_format12(&[(0x00E9, 0x00E9, 42)]);
        let t = Reader::new(&bytes);
        assert_eq!(lookup_format12(&t, 'é' as u32), Some(42));
    }
}
