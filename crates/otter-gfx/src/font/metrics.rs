//! `hmtx`: per-glyph advance widths.

use super::tables::Reader;

/// The glyph's advance width, in font units. Glyphs beyond the `hmtx` array's
/// explicit entries repeat the last explicit advance width (as the spec requires
/// for monospace-tail optimization). Malformed/truncated data yields `0` rather
/// than an error: a missing advance is not worth failing an entire line of text.
pub fn advance_width(hmtx: &Reader, num_h_metrics: u16, _num_glyphs: u16, glyph_id: u16) -> u16 {
    if num_h_metrics == 0 {
        return 0;
    }
    let idx = glyph_id.min(num_h_metrics - 1);
    hmtx.u16(idx as usize * 4).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn build_hmtx(entries: &[(u16, i16)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(advance, lsb) in entries {
            out.extend_from_slice(&advance.to_be_bytes());
            out.extend_from_slice(&lsb.to_be_bytes());
        }
        out
    }

    #[test]
    fn reads_explicit_advances() {
        let bytes = build_hmtx(&[(600, 10), (700, 20), (800, 0)]);
        let t = Reader::new(&bytes);
        assert_eq!(advance_width(&t, 3, 3, 0), 600);
        assert_eq!(advance_width(&t, 3, 3, 1), 700);
        assert_eq!(advance_width(&t, 3, 3, 2), 800);
    }

    #[test]
    fn glyphs_past_num_h_metrics_repeat_the_last_advance() {
        let bytes = build_hmtx(&[(600, 10), (900, 0)]);
        let t = Reader::new(&bytes);
        assert_eq!(advance_width(&t, 2, 10, 5), 900);
        assert_eq!(advance_width(&t, 2, 10, 9), 900);
    }

    #[test]
    fn zero_metrics_is_zero_not_panic() {
        let bytes: Vec<u8> = Vec::new();
        let t = Reader::new(&bytes);
        assert_eq!(advance_width(&t, 0, 0, 0), 0);
    }
}
