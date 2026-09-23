//! `GPOS` pair adjustment (lookup type 2, subtable formats 1 and 2): the kerning
//! path most modern TrueType fonts actually use. Only the default script/language
//! system's `kern` feature is consulted, and only each pair's first glyph's
//! x-advance adjustment is read (exactly what horizontal kerning needs).

use alloc::vec::Vec;

use super::tables::Reader;
use super::FontError;

/// Walks Script/Feature/Lookup lists to collect every lookup-type-2 (pair
/// adjustment) subtable reachable from the `kern` feature under the default
/// script's default language system.
pub fn collect_kern_pair_subtables(gpos: Reader<'_>) -> Result<Vec<Reader<'_>>, FontError> {
    let script_list_off = gpos.u16(4)? as usize;
    let feature_list_off = gpos.u16(6)? as usize;
    let lookup_list_off = gpos.u16(8)? as usize;

    let script_list = gpos.sub(script_list_off, gpos.len() - script_list_off)?;
    let Some(lang_sys) = find_default_lang_sys(&script_list)? else {
        return Ok(Vec::new());
    };

    let feature_list = gpos.sub(feature_list_off, gpos.len() - feature_list_off)?;
    let kern_features = find_kern_features(&lang_sys, &feature_list)?;

    let mut lookup_indices = Vec::new();
    for feat_idx in kern_features {
        collect_lookup_indices(&feature_list, feat_idx, &mut lookup_indices)?;
    }

    let lookup_list = gpos.sub(lookup_list_off, gpos.len() - lookup_list_off)?;
    let lookup_count = lookup_list.u16(0)?;
    let mut subtables = Vec::new();
    for li in lookup_indices {
        if li >= lookup_count {
            continue;
        }
        let lookup_off = lookup_list.u16(2 + li as usize * 2)? as usize;
        let lookup = lookup_list.sub(lookup_off, lookup_list.len() - lookup_off)?;
        let lookup_type = lookup.u16(0)?;
        // Lookup type 9 is "Extension Positioning": each of its "subtables" is
        // actually a small indirection record pointing at the real subtable, of
        // whatever type it declares (almost always 2, for kerning) -- used so a
        // large table like a full kerning set can use 32-bit offsets. Fonts built
        // by modern pipelines (Inter included) commonly wrap all of GPOS this way.
        if lookup_type != 2 && lookup_type != 9 {
            continue;
        }
        let sub_count = lookup.u16(4)?;
        for s in 0..sub_count {
            let sub_off = lookup.u16(6 + s as usize * 2)? as usize;
            let sub_reader = lookup.sub(sub_off, lookup.len() - sub_off)?;
            if lookup_type == 2 {
                subtables.push(sub_reader);
                continue;
            }
            if sub_reader.u16(0) != Ok(1) || sub_reader.u16(2) != Ok(2) {
                continue; // not ExtensionPosFormat1, or doesn't wrap a PairPos
            }
            let Ok(ext_offset) = sub_reader.u32(4) else { continue };
            if let Ok(real_sub) = sub_reader.sub(ext_offset as usize, sub_reader.len().saturating_sub(ext_offset as usize)) {
                subtables.push(real_sub);
            }
        }
    }
    Ok(subtables)
}

fn find_default_lang_sys<'a>(script_list: &Reader<'a>) -> Result<Option<Reader<'a>>, FontError> {
    let script_count = script_list.u16(0)?;
    let mut chosen: Option<usize> = None;
    let mut fallback: Option<usize> = None;
    for i in 0..script_count {
        let rec = 2 + i as usize * 6;
        let tag = script_list.tag(rec)?;
        let off = script_list.u16(rec + 4)? as usize;
        if &tag == b"DFLT" {
            chosen = Some(off);
            break;
        }
        if &tag == b"latn" && chosen.is_none() {
            chosen = Some(off);
        }
        if fallback.is_none() {
            fallback = Some(off);
        }
    }
    let Some(script_off) = chosen.or(fallback) else {
        return Ok(None);
    };
    let script_table = script_list.sub(script_off, script_list.len() - script_off)?;
    let default_lang_sys_off = script_table.u16(0)? as usize;
    if default_lang_sys_off == 0 {
        return Ok(None);
    }
    Ok(Some(script_table.sub(default_lang_sys_off, script_table.len() - default_lang_sys_off)?))
}

fn find_kern_features(lang_sys: &Reader, feature_list: &Reader) -> Result<Vec<u16>, FontError> {
    let feature_index_count = lang_sys.u16(4)?;
    let feature_count = feature_list.u16(0)?;
    let mut result = Vec::new();
    for i in 0..feature_index_count {
        let feat_idx = lang_sys.u16(6 + i as usize * 2)?;
        if feat_idx >= feature_count {
            continue;
        }
        let rec = 2 + feat_idx as usize * 6;
        if &feature_list.tag(rec)? == b"kern" {
            result.push(feat_idx);
        }
    }
    Ok(result)
}

fn collect_lookup_indices(feature_list: &Reader, feat_idx: u16, out: &mut Vec<u16>) -> Result<(), FontError> {
    let feature_off = feature_list.u16(2 + feat_idx as usize * 6 + 4)? as usize;
    let feature = feature_list.sub(feature_off, feature_list.len() - feature_off)?;
    let lookup_index_count = feature.u16(2)?;
    for i in 0..lookup_index_count {
        out.push(feature.u16(4 + i as usize * 2)?);
    }
    Ok(())
}

fn value_record_size(format: u16) -> usize {
    format.count_ones() as usize * 2
}

/// Reads just the `XAdvance` field (if present) out of a `ValueRecord` starting at
/// `base`, honouring the fixed field order (`XPlacement, YPlacement, XAdvance,
/// YAdvance, ...device tables`) so fields present before it are correctly skipped.
fn read_x_advance(r: &Reader, base: usize, format: u16) -> Option<i16> {
    if format & 0x0004 == 0 {
        return Some(0);
    }
    let mut off = base;
    if format & 0x0001 != 0 {
        off += 2;
    }
    if format & 0x0002 != 0 {
        off += 2;
    }
    r.i16(off).ok()
}

fn coverage_index(cov: &Reader, glyph: u16) -> Option<u32> {
    let format = cov.u16(0).ok()?;
    match format {
        1 => {
            let count = cov.u16(2).ok()? as i64;
            let mut lo: i64 = 0;
            let mut hi: i64 = count - 1;
            while lo <= hi {
                let mid = (lo + hi) / 2;
                let g = cov.u16(4 + mid as usize * 2).ok()?;
                if g == glyph {
                    return Some(mid as u32);
                } else if g < glyph {
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            None
        }
        2 => {
            let range_count = cov.u16(2).ok()?;
            for i in 0..range_count {
                let rec = 4 + i as usize * 6;
                let start = cov.u16(rec).ok()?;
                let end = cov.u16(rec + 2).ok()?;
                if glyph >= start && glyph <= end {
                    let start_cov_index = cov.u16(rec + 4).ok()?;
                    return Some(start_cov_index as u32 + (glyph - start) as u32);
                }
            }
            None
        }
        _ => None,
    }
}

fn class_of(class_def: &Reader, glyph: u16) -> u16 {
    let Ok(format) = class_def.u16(0) else { return 0 };
    match format {
        1 => {
            let (Ok(start), Ok(count)) = (class_def.u16(2), class_def.u16(4)) else { return 0 };
            if glyph < start || glyph >= start.saturating_add(count) {
                return 0;
            }
            class_def.u16(6 + (glyph - start) as usize * 2).unwrap_or(0)
        }
        2 => {
            let Ok(range_count) = class_def.u16(2) else { return 0 };
            for i in 0..range_count {
                let rec = 4 + i as usize * 6;
                let (Ok(start), Ok(end)) = (class_def.u16(rec), class_def.u16(rec + 2)) else { return 0 };
                if glyph >= start && glyph <= end {
                    return class_def.u16(rec + 4).unwrap_or(0);
                }
            }
            0
        }
        _ => 0,
    }
}

fn lookup_pair_format1(sub: &Reader, left: u16, right: u16) -> Option<i16> {
    let coverage_off = sub.u16(2).ok()? as usize;
    let value_format1 = sub.u16(4).ok()?;
    let value_format2 = sub.u16(6).ok()?;
    let pair_set_count = sub.u16(8).ok()?;
    let coverage = sub.sub(coverage_off, sub.len() - coverage_off).ok()?;
    let cov_index = coverage_index(&coverage, left)?;
    if cov_index as u16 >= pair_set_count {
        return None;
    }
    let pair_set_off = sub.u16(10 + cov_index as usize * 2).ok()? as usize;
    let pair_set = sub.sub(pair_set_off, sub.len() - pair_set_off).ok()?;
    let pair_value_count = pair_set.u16(0).ok()?;
    let rec_size = 2 + value_record_size(value_format1) + value_record_size(value_format2);
    for i in 0..pair_value_count {
        let rec_off = 2 + i as usize * rec_size;
        let second_glyph = pair_set.u16(rec_off).ok()?;
        if second_glyph == right {
            return read_x_advance(&pair_set, rec_off + 2, value_format1);
        }
        if second_glyph > right {
            break; // PairValueRecords are sorted by secondGlyph
        }
    }
    None
}

fn lookup_pair_format2(sub: &Reader, left: u16, right: u16) -> Option<i16> {
    let coverage_off = sub.u16(2).ok()? as usize;
    let value_format1 = sub.u16(4).ok()?;
    let value_format2 = sub.u16(6).ok()?;
    let class_def1_off = sub.u16(8).ok()? as usize;
    let class_def2_off = sub.u16(10).ok()? as usize;
    let class1_count = sub.u16(12).ok()?;
    let class2_count = sub.u16(14).ok()?;

    let coverage = sub.sub(coverage_off, sub.len() - coverage_off).ok()?;
    coverage_index(&coverage, left)?; // subtable only applies if `left` is covered

    let class_def1 = sub.sub(class_def1_off, sub.len() - class_def1_off).ok()?;
    let class_def2 = sub.sub(class_def2_off, sub.len() - class_def2_off).ok()?;
    let class1 = class_of(&class_def1, left);
    let class2 = class_of(&class_def2, right);
    if class1 >= class1_count || class2 >= class2_count {
        return None;
    }

    let rec_size = value_record_size(value_format1) + value_record_size(value_format2);
    let base = 16 + (class1 as usize * class2_count as usize + class2 as usize) * rec_size;
    read_x_advance(sub, base, value_format1)
}

/// Looks up a pair-adjustment subtable (format 1 or 2) for the x-advance kerning
/// between `left` and `right`, or `None` if this subtable has no opinion about
/// that pair.
pub fn lookup_pair(sub: &Reader, left: u16, right: u16) -> Option<i16> {
    match sub.u16(0).ok()? {
        1 => lookup_pair_format1(sub, left, right),
        2 => lookup_pair_format2(sub, left, right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_coverage_format1(glyphs: &[u16]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(glyphs.len() as u16).to_be_bytes());
        for &g in glyphs {
            out.extend_from_slice(&g.to_be_bytes());
        }
        out
    }

    /// Builds a minimal PairPos format-1 subtable with one covered left glyph and
    /// one pair, XAdvance-only value records.
    fn build_pair_pos_format1(left: u16, right: u16, x_advance: i16) -> Vec<u8> {
        // Layout: [0]posFormat [2]coverageOffset [4]valueFormat1 [6]valueFormat2
        // [8]pairSetCount [10]pairSetOffsets[] ... coverage ... pairSet
        let value_format1 = 0x0004u16; // XAdvance only
        let value_format2 = 0u16;
        let header_len = 10 + 2; // one pairSetOffset
        let coverage = build_coverage_format1(&[left]);
        let pair_set_off = header_len + coverage.len();
        let mut pair_set = Vec::new();
        pair_set.extend_from_slice(&1u16.to_be_bytes()); // pairValueCount
        pair_set.extend_from_slice(&right.to_be_bytes());
        pair_set.extend_from_slice(&x_advance.to_be_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(&1u16.to_be_bytes()); // posFormat
        out.extend_from_slice(&(header_len as u16).to_be_bytes()); // coverageOffset
        out.extend_from_slice(&value_format1.to_be_bytes());
        out.extend_from_slice(&value_format2.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes()); // pairSetCount
        out.extend_from_slice(&(pair_set_off as u16).to_be_bytes());
        out.extend_from_slice(&coverage);
        out.extend_from_slice(&pair_set);
        out
    }

    #[test]
    fn pair_pos_format1_reads_x_advance() {
        let bytes = build_pair_pos_format1(10, 20, -80);
        let t = Reader::new(&bytes);
        assert_eq!(lookup_pair(&t, 10, 20), Some(-80));
        assert_eq!(lookup_pair(&t, 10, 21), None);
        assert_eq!(lookup_pair(&t, 11, 20), None);
    }

    fn build_class_def_format2(ranges: &[(u16, u16, u16)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&2u16.to_be_bytes());
        out.extend_from_slice(&(ranges.len() as u16).to_be_bytes());
        for &(s, e, c) in ranges {
            out.extend_from_slice(&s.to_be_bytes());
            out.extend_from_slice(&e.to_be_bytes());
            out.extend_from_slice(&c.to_be_bytes());
        }
        out
    }

    /// Builds a PairPos format-2 subtable: class1 in {0,1}, class2 in {0,1}, with
    /// XAdvance-only records for every (class1,class2) combination.
    fn build_pair_pos_format2(covered_left: &[u16], class_def1: &[u8], class_def2: &[u8], values: &[[i16; 2]; 2]) -> Vec<u8> {
        let coverage = build_coverage_format1(covered_left);
        let header_len = 16usize;
        // The Class1Records array (class1Count * class2Count value-record pairs)
        // sits immediately after the 16-byte header, *before* ClassDef1/2/Coverage.
        let value_bytes = 2 * 2 * 2; // class1Count * class2Count * one XAdvance (2 bytes)
        let class_def1_off = header_len + value_bytes;
        let class_def2_off = class_def1_off + class_def1.len();
        let coverage_off = class_def2_off + class_def2.len();

        let mut out = Vec::new();
        out.extend_from_slice(&2u16.to_be_bytes()); // posFormat
        out.extend_from_slice(&(coverage_off as u16).to_be_bytes());
        out.extend_from_slice(&0x0004u16.to_be_bytes()); // valueFormat1 = XAdvance
        out.extend_from_slice(&0u16.to_be_bytes()); // valueFormat2
        out.extend_from_slice(&(class_def1_off as u16).to_be_bytes());
        out.extend_from_slice(&(class_def2_off as u16).to_be_bytes());
        out.extend_from_slice(&2u16.to_be_bytes()); // class1Count
        out.extend_from_slice(&2u16.to_be_bytes()); // class2Count
        for row in values {
            for &v in row {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        out.extend_from_slice(class_def1);
        out.extend_from_slice(class_def2);
        out.extend_from_slice(&coverage);
        out
    }

    #[test]
    fn pair_pos_format2_reads_class_based_x_advance() {
        let class_def1 = build_class_def_format2(&[(10, 10, 1)]); // glyph 10 -> class1
        let class_def2 = build_class_def_format2(&[(20, 20, 1)]); // glyph 20 -> class1
        let values = [[0i16, -10], [-20, -99]];
        let bytes = build_pair_pos_format2(&[10, 11], &class_def1, &class_def2, &values);
        let t = Reader::new(&bytes);
        // left=10 (class1), right=20 (class1) -> values[1][1] = -99
        assert_eq!(lookup_pair(&t, 10, 20), Some(-99));
        // left=11 (class0, covered), right=99 (class0) -> values[0][0] = 0
        assert_eq!(lookup_pair(&t, 11, 99), Some(0));
        // left=99 is not covered at all
        assert_eq!(lookup_pair(&t, 99, 20), None);
    }

    #[test]
    fn coverage_format2_ranges_resolve_index() {
        let mut cov = Vec::new();
        cov.extend_from_slice(&2u16.to_be_bytes());
        cov.extend_from_slice(&1u16.to_be_bytes()); // rangeCount
        cov.extend_from_slice(&100u16.to_be_bytes()); // startGlyph
        cov.extend_from_slice(&110u16.to_be_bytes()); // endGlyph
        cov.extend_from_slice(&0u16.to_be_bytes()); // startCoverageIndex
        let t = Reader::new(&cov);
        assert_eq!(coverage_index(&t, 100), Some(0));
        assert_eq!(coverage_index(&t, 105), Some(5));
        assert_eq!(coverage_index(&t, 111), None);
    }
}
