//! The legacy `kern` table, format 0 subtables only (ordered pair -> value).

use super::tables::Reader;
use super::FontError;

/// Finds the first version-0, format-0 subtable in a `kern` table, if any (other
/// subtable formats -- and the Apple `kern` version-1 header layout -- are not
/// used for pair kerning by any font we target, so they're skipped rather than
/// treated as an error).
pub fn find_format0<'a>(kern_table: Reader<'a>) -> Result<Option<Reader<'a>>, FontError> {
    let version = kern_table.u16(0)?;
    if version != 0 {
        return Ok(None);
    }
    let n_tables = kern_table.u16(2)?;
    let mut off = 4usize;
    for _ in 0..n_tables {
        let sub_version = kern_table.u16(off)?;
        let length = kern_table.u16(off + 2)? as usize;
        let coverage = kern_table.u16(off + 4)?;
        let format = coverage >> 8;
        if length < 6 {
            break; // malformed: cannot contain even the subtable header
        }
        if format == 0 && sub_version == 0 {
            return Ok(Some(kern_table.sub(off + 6, length - 6)?));
        }
        off += length;
    }
    Ok(None)
}

/// Binary-searches a format-0 subtable body (i.e. what [`find_format0`] returns)
/// for the kerning value between an ordered `(left, right)` glyph pair.
pub fn lookup(sub: &Reader, left: u16, right: u16) -> Option<i16> {
    let n_pairs = sub.u16(0).ok()? as i64;
    let key = ((left as u32) << 16) | right as u32;
    let mut lo: i64 = 0;
    let mut hi: i64 = n_pairs - 1;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let rec = 8 + mid as usize * 6;
        let l = sub.u16(rec).ok()?;
        let r = sub.u16(rec + 2).ok()?;
        let k = ((l as u32) << 16) | r as u32;
        if k == key {
            return sub.i16(rec + 4).ok();
        } else if k < key {
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn build_kern_table(pairs: &[(u16, u16, i16)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&1u16.to_be_bytes()); // nTables
        let sub_len = 14 + pairs.len() * 6;
        out.extend_from_slice(&0u16.to_be_bytes()); // subtable version
        out.extend_from_slice(&(sub_len as u16).to_be_bytes());
        out.extend_from_slice(&0x0000u16.to_be_bytes()); // coverage: format 0, horizontal
        out.extend_from_slice(&(pairs.len() as u16).to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
        out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        for &(l, r, v) in pairs {
            out.extend_from_slice(&l.to_be_bytes());
            out.extend_from_slice(&r.to_be_bytes());
            out.extend_from_slice(&v.to_be_bytes());
        }
        out
    }

    #[test]
    fn finds_and_looks_up_a_pair() {
        let bytes = build_kern_table(&[(1, 2, -50), (1, 5, 10), (9, 9, 3)]);
        let t = Reader::new(&bytes);
        let sub = find_format0(t).unwrap().expect("subtable present");
        assert_eq!(lookup(&sub, 1, 2), Some(-50));
        assert_eq!(lookup(&sub, 1, 5), Some(10));
        assert_eq!(lookup(&sub, 9, 9), Some(3));
    }

    #[test]
    fn missing_pair_is_none() {
        let bytes = build_kern_table(&[(1, 2, -50)]);
        let t = Reader::new(&bytes);
        let sub = find_format0(t).unwrap().unwrap();
        assert_eq!(lookup(&sub, 3, 4), None);
    }

    #[test]
    fn non_zero_version_is_not_format0() {
        let mut bytes = build_kern_table(&[(1, 2, -50)]);
        bytes[1] = 1; // version = 1 (Apple AAT layout, not supported)
        let t = Reader::new(&bytes);
        assert!(find_format0(t).unwrap().is_none());
    }
}
