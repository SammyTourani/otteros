//! A from-scratch TrueType (`glyf`) font parser: table directory, `cmap` (formats 4
//! and 12), `hhea`/`hmtx` metrics, `loca`/`glyf` outlines (simple and composite),
//! and kerning from either the legacy `kern` table (format 0) or `GPOS` pair
//! adjustment (lookup type 2, formats 1 and 2). CFF-flavoured (`OTTO`) fonts are
//! rejected with a clear error, never guessed at.
//!
//! Every multi-byte read goes through [`tables::Reader`], which never panics on
//! out-of-range input -- malformed or truncated fonts always produce a
//! [`FontError`], which is what lets `text::` fuzz malformed fonts safely.

mod cmap;
mod glyf;
mod gpos;
mod kern;
mod metrics;
mod tables;

use alloc::vec::Vec;

use crate::path::Path;
use tables::Reader;

/// Everything that can go wrong parsing or querying a font.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontError {
    /// A read ran past the end of the font data or a table.
    Truncated,
    /// The sfnt version was neither `0x00010000` nor `'true'`.
    BadSfntVersion,
    /// The font is CFF-flavoured (`OTTO`): PostScript outlines are not supported.
    CffNotSupported,
    /// A table required to render text at all was missing.
    MissingTable(&'static str),
    /// A glyph ID was outside `0..numGlyphs`.
    InvalidGlyphId,
    /// The `glyf`/`loca` data for a glyph was internally inconsistent.
    MalformedGlyf,
    /// A composite glyph nested more components than [`glyf::MAX_COMPOSITE_DEPTH`]
    /// deep, or listed more than [`glyf::MAX_COMPONENTS`] components.
    CompositeRecursionLimit,
    /// No `cmap` subtable in a format this parser understands (4 or 12) was found.
    NoUsableCmap,
}

/// A parsed TrueType font, borrowing its backing bytes.
pub struct Font<'a> {
    units_per_em: u16,
    ascender: i16,
    descender: i16,
    line_gap: i16,
    bbox: (i16, i16, i16, i16),
    num_glyphs: u16,
    loca_long: bool,
    glyf: Reader<'a>,
    loca: Reader<'a>,
    hmtx: Reader<'a>,
    num_h_metrics: u16,
    cmap: cmap::CmapSubtable<'a>,
    kern: Option<Reader<'a>>,
    gpos_pair_subtables: Vec<Reader<'a>>,
}

fn find_table<'a>(r: &Reader<'a>, tag: &[u8; 4]) -> Result<Option<(u32, u32)>, FontError> {
    let num_tables = r.u16(4)?;
    for i in 0..num_tables {
        let rec_off = 12 + i as usize * 16;
        if r.tag(rec_off)? == *tag {
            let offset = r.u32(rec_off + 8)?;
            let length = r.u32(rec_off + 12)?;
            return Ok(Some((offset, length)));
        }
    }
    Ok(None)
}

impl<'a> Font<'a> {
    /// Parses an sfnt (TrueType) font from raw file bytes.
    pub fn parse(data: &'a [u8]) -> Result<Self, FontError> {
        let r = Reader::new(data);
        let version = r.u32(0)?;
        if version == 0x4F54_544F {
            return Err(FontError::CffNotSupported);
        }
        if version != 0x0001_0000 && version != 0x7472_7565 {
            return Err(FontError::BadSfntVersion);
        }

        let required = |tag: &[u8; 4], name: &'static str| -> Result<(u32, u32), FontError> {
            find_table(&r, tag)?.ok_or(FontError::MissingTable(name))
        };

        let (head_off, head_len) = required(b"head", "head")?;
        let head = r.sub(head_off as usize, head_len as usize)?;
        let units_per_em = head.u16(18)?;
        let bbox = (head.i16(36)?, head.i16(38)?, head.i16(40)?, head.i16(42)?);
        let loca_long = head.i16(50)? != 0;

        let (maxp_off, maxp_len) = required(b"maxp", "maxp")?;
        let num_glyphs = r.sub(maxp_off as usize, maxp_len as usize)?.u16(4)?;

        let (hhea_off, hhea_len) = required(b"hhea", "hhea")?;
        let hhea = r.sub(hhea_off as usize, hhea_len as usize)?;
        let ascender = hhea.i16(4)?;
        let descender = hhea.i16(6)?;
        let line_gap = hhea.i16(8)?;
        let num_h_metrics = hhea.u16(34)?;

        let (hmtx_off, hmtx_len) = required(b"hmtx", "hmtx")?;
        let hmtx = r.sub(hmtx_off as usize, hmtx_len as usize)?;

        let (loca_off, loca_len) = required(b"loca", "loca")?;
        let loca = r.sub(loca_off as usize, loca_len as usize)?;

        let (glyf_off, glyf_len) = required(b"glyf", "glyf")?;
        let glyf = r.sub(glyf_off as usize, glyf_len as usize)?;

        let (cmap_off, cmap_len) = required(b"cmap", "cmap")?;
        let cmap = cmap::parse_best_subtable(r.sub(cmap_off as usize, cmap_len as usize)?)?;

        let kern = find_table(&r, b"kern")?
            .and_then(|(o, l)| r.sub(o as usize, l as usize).ok())
            .and_then(|t| kern::find_format0(t).unwrap_or(None));

        let gpos_pair_subtables = match find_table(&r, b"GPOS")? {
            Some((o, l)) => r
                .sub(o as usize, l as usize)
                .ok()
                .and_then(|t| gpos::collect_kern_pair_subtables(t).ok())
                .unwrap_or_default(),
            None => Vec::new(),
        };

        Ok(Font {
            units_per_em,
            ascender,
            descender,
            line_gap,
            bbox,
            num_glyphs,
            loca_long,
            glyf,
            loca,
            hmtx,
            num_h_metrics,
            cmap,
            kern,
            gpos_pair_subtables,
        })
    }

    /// Units per em (the scale of every other font-unit value here).
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// Recommended ascender, in font units above the baseline.
    pub fn ascender(&self) -> i16 {
        self.ascender
    }

    /// Recommended descender, in font units below the baseline (negative).
    pub fn descender(&self) -> i16 {
        self.descender
    }

    /// Recommended extra line spacing, in font units.
    pub fn line_gap(&self) -> i16 {
        self.line_gap
    }

    /// Total number of glyphs in the font.
    pub fn num_glyphs(&self) -> u16 {
        self.num_glyphs
    }

    /// The font-wide glyph bounding box from `head` (`xMin, yMin, xMax, yMax`, font
    /// units): every individual glyph's own outline bounds must fit inside this.
    pub fn bbox(&self) -> (i16, i16, i16, i16) {
        self.bbox
    }

    /// Number of components in a composite glyph (`1` for a simple glyph, `0` for
    /// an empty one like space).
    pub fn component_count(&self, glyph_id: u16) -> Result<usize, FontError> {
        glyf::count_components(&self.glyf, &self.loca, self.loca_long, self.num_glyphs, glyph_id)
    }

    /// Looks up the glyph ID for a Unicode scalar value, or `0` (`.notdef`) if the
    /// font's `cmap` has no mapping for it.
    pub fn glyph_id_for_char(&self, c: char) -> u16 {
        cmap::lookup(&self.cmap, c as u32).unwrap_or(0)
    }

    /// The glyph's outline, in font units (y-up, as TrueType defines it).
    pub fn outline(&self, glyph_id: u16) -> Result<Path, FontError> {
        glyf::parse_glyph(&self.glyf, &self.loca, self.loca_long, self.num_glyphs, glyph_id, 0)
    }

    /// The glyph's advance width, in font units.
    pub fn advance_width(&self, glyph_id: u16) -> u16 {
        metrics::advance_width(&self.hmtx, self.num_h_metrics, self.num_glyphs, glyph_id)
    }

    /// The kerning adjustment (font units, usually negative) to add between two
    /// consecutive glyphs: `GPOS` pair adjustment first (formats 1 and 2), falling
    /// back to the legacy `kern` table (format 0), or `0` if neither has an entry.
    pub fn kerning(&self, left: u16, right: u16) -> i16 {
        for sub in &self.gpos_pair_subtables {
            if let Some(v) = gpos::lookup_pair(sub, left, right) {
                return v;
            }
        }
        if let Some(k) = &self.kern
            && let Some(v) = kern::lookup(k, left, right)
        {
            return v;
        }
        0
    }
}
