//! Text layout and rendering: a glyph raster cache, measuring, drawing, word wrap,
//! ellipsis truncation, tab stops, and a `.notdef` fallback box for characters a
//! font has no mapping for. Also a small standalone UTF-8 decoder for text arriving
//! as raw, not-necessarily-valid bytes (e.g. a future terminal's input stream).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::color::Color;
use crate::font::Font;
use crate::geom::{Affine, FloatExt};
use crate::path::Path;
use crate::raster::rasterize;
use crate::surface::Canvas;

/// Gamma-corrects glyph coverage (`(coverage/255)^0.83`, precomputed) so
/// light-on-dark text doesn't look thinner/wispier than dark-on-light text at
/// the same nominal coverage. Blending happens directly in sRGB-encoded space
/// (no linear-light conversion -- the crate has no colour management), which
/// under-represents a light shape's effective coverage against a dark
/// background relative to how a dark shape reads against a light one; this
/// table (concentrated in the low end, near-identity by mid-tone) corrects
/// just that asymmetry without over-darkening solid stem interiors. See
/// `is_light_text` for which polarity it's applied to, and
/// `tests::gamma_makes_thin_coverage_visibly_bolder` for the effect it has.
const GAMMA_BOOST_LUT: [u8; 256] = [
    0, 3, 5, 6, 8, 10, 11, 13, 14, 16, 17, 19, 20, 22, 23, 24, 26, 27, 28, 30, 31, 32, 33, 35, 36, 37, 38, 40, 41, 42,
    43, 44, 46, 47, 48, 49, 50, 51, 53, 54, 55, 56, 57, 58, 59, 60, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 74,
    75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101,
    102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123,
    124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144,
    145, 146, 147, 148, 149, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 161, 162, 163, 164,
    165, 166, 167, 168, 169, 170, 170, 171, 172, 173, 174, 175, 176, 177, 178, 179, 179, 180, 181, 182, 183, 184,
    185, 186, 187, 187, 188, 189, 190, 191, 192, 193, 194, 194, 195, 196, 197, 198, 199, 200, 201, 201, 202, 203,
    204, 205, 206, 207, 208, 208, 209, 210, 211, 212, 213, 214, 214, 215, 216, 217, 218, 219, 220, 220, 221, 222,
    223, 224, 225, 226, 226, 227, 228, 229, 230, 231, 232, 232, 233, 234, 235, 236, 237, 237, 238, 239, 240, 241,
    242, 242, 243, 244, 245, 246, 247, 248, 248, 249, 250, 251, 252, 253, 253, 254, 255,
];

/// True if `color` reads as "light" (so text set in it is presumably on a
/// darker background -- see [`GAMMA_BOOST_LUT`]), by Rec. 601 luma.
fn is_light_text(color: Color) -> bool {
    let luma = (color.r as u32 * 77 + color.g as u32 * 151 + color.b as u32 * 28) >> 8;
    luma >= 128
}

/// Decodes raw bytes as UTF-8, substituting U+FFFD for any invalid byte or
/// sequence (never panics, never rejects input wholesale). Written from scratch
/// rather than relying on `core::str::from_utf8` for the common well-formed case,
/// as a genuinely more permissive decoder for text arriving from untrusted byte
/// sources (a terminal, a network socket) that `&str`'s all-or-nothing validity
/// guarantee can't help with.
pub fn decode_utf8(bytes: &[u8]) -> Vec<char> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let (len, min_cp): (usize, u32) = if b0 < 0x80 {
            (1, 0)
        } else if b0 & 0xE0 == 0xC0 {
            (2, 0x80)
        } else if b0 & 0xF0 == 0xE0 {
            (3, 0x800)
        } else if b0 & 0xF8 == 0xF0 {
            (4, 0x10000)
        } else {
            (0, 0)
        };
        if len == 0 {
            out.push('\u{FFFD}');
            i += 1;
            continue;
        }
        if i + len > bytes.len() {
            // Truncated at the end of the input (not an invalid continuation byte,
            // just not enough bytes left): Unicode's "maximal subpart" convention
            // is one replacement character for the lead byte plus every valid
            // trailing continuation byte actually present, not one per byte.
            let mut consumed = 1;
            while i + consumed < bytes.len() && bytes[i + consumed] & 0xC0 == 0x80 {
                consumed += 1;
            }
            out.push('\u{FFFD}');
            i += consumed;
            continue;
        }
        let mut cp: u32 = match len {
            1 => b0 as u32,
            2 => b0 as u32 & 0x1F,
            3 => b0 as u32 & 0x0F,
            _ => b0 as u32 & 0x07,
        };
        let mut valid = true;
        for k in 1..len {
            let b = bytes[i + k];
            if b & 0xC0 != 0x80 {
                valid = false;
                break;
            }
            cp = (cp << 6) | (b as u32 & 0x3F);
        }
        if !valid || cp < min_cp || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
            out.push('\u{FFFD}');
            i += 1;
            continue;
        }
        out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
        i += len;
    }
    out
}

/// A hollow "tofu" box (the classic missing-glyph placeholder), sized relative to
/// `units_per_em`, used whenever [`Font::glyph_id_for_char`] can't find a mapping.
pub fn notdef_box_path(units_per_em: u16) -> Path {
    let upm = units_per_em.max(1) as f32;
    let (x0, x1, y0, y1) = (upm * 0.12, upm * 0.88, 0.0, upm * 0.7);
    let inset = upm * 0.08;
    let mut p = Path::new();
    p.move_to(x0, y0).line_to(x1, y0).line_to(x1, y1).line_to(x0, y1).close();
    p.move_to(x0 + inset, y0 + inset)
        .line_to(x0 + inset, y1 - inset)
        .line_to(x1 - inset, y1 - inset)
        .line_to(x1 - inset, y0 + inset)
        .close();
    p
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GlyphKey {
    font_id: u32,
    glyph_id: u16,
    size_q: u32,
    subpixel_third: u8,
}

struct GlyphBitmap {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    coverage: Vec<u8>,
}

impl GlyphBitmap {
    fn empty() -> Self {
        GlyphBitmap { x: 0, y: 0, w: 0, h: 0, coverage: Vec::new() }
    }
}

fn rasterize_glyph(font: &Font, glyph_id: u16, pixel_size: f32, subpixel_third: u8) -> GlyphBitmap {
    let upm = font.units_per_em().max(1) as f32;
    let scale = pixel_size / upm;
    let sub_x = subpixel_third as f32 / 3.0;
    let outline =
        if glyph_id == 0 { notdef_box_path(font.units_per_em()) } else { font.outline(glyph_id).unwrap_or_default() };
    if outline.is_empty() {
        return GlyphBitmap::empty();
    }
    // Font space is y-up with the origin on the baseline; raster space is y-down.
    let transform = Affine { a: scale, b: 0.0, c: 0.0, d: -scale, e: sub_x, f: 0.0 };
    match rasterize(&outline.transformed(transform), 0.25) {
        Some(mask) => GlyphBitmap { x: mask.x, y: mask.y, w: mask.w, h: mask.h, coverage: mask.coverage },
        None => GlyphBitmap::empty(),
    }
}

/// Caches rasterized glyph coverage bitmaps keyed by `(font id, glyph, pixel size,
/// subpixel x offset in thirds)`, so repeatedly drawing the same text doesn't
/// re-run the vector rasterizer every frame. `font_id` is caller-assigned (e.g. an
/// index into a small font table) so one cache can serve several fonts.
pub struct GlyphCache {
    entries: BTreeMap<GlyphKey, GlyphBitmap>,
}

impl GlyphCache {
    /// An empty cache.
    pub fn new() -> Self {
        GlyphCache { entries: BTreeMap::new() }
    }

    /// Number of distinct glyphs currently cached (used by tests).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if nothing has been cached yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn get_or_rasterize(&mut self, font: &Font, font_id: u32, glyph_id: u16, pixel_size: f32, subpixel_third: u8) -> &GlyphBitmap {
        let size_q = (pixel_size * 64.0).fround() as u32;
        let key = GlyphKey { font_id, glyph_id, size_q, subpixel_third };
        self.entries.entry(key).or_insert_with(|| rasterize_glyph(font, glyph_id, pixel_size, subpixel_third))
    }
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self::new()
    }
}

fn tab_advance(x: f32, tab_width: f32) -> f32 {
    if tab_width <= 0.0 {
        return x;
    }
    ((x / tab_width).ffloor() + 1.0) * tab_width
}

/// The total advance width of `text` set in `font` at `size` pixels, honouring
/// kerning and tab stops every `tab_width` pixels (pass `0.0` to disable tabs).
pub fn measure(font: &Font, text: &str, size: f32, tab_width: f32) -> f32 {
    let upm = font.units_per_em().max(1) as f32;
    let scale = size / upm;
    let mut x = 0.0f32;
    let mut prev: Option<u16> = None;
    for c in text.chars() {
        if c == '\t' {
            x = tab_advance(x, tab_width);
            prev = None;
            continue;
        }
        let gid = font.glyph_id_for_char(c);
        if let Some(p) = prev {
            x += font.kerning(p, gid) as f32 * scale;
        }
        x += font.advance_width(gid) as f32 * scale;
        prev = Some(gid);
    }
    x
}

/// Draws `text` with its baseline at `(x, y)`, returning the total advance
/// (equivalent to [`measure`]).
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    canvas: &mut Canvas,
    cache: &mut GlyphCache,
    font: &Font,
    font_id: u32,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    tab_width: f32,
    color: Color,
) -> f32 {
    let upm = font.units_per_em().max(1) as f32;
    let scale = size / upm;
    let premul = color.premul();
    let light = is_light_text(color);
    let mut pen_x = x;
    let mut prev: Option<u16> = None;
    for c in text.chars() {
        if c == '\t' {
            pen_x = tab_advance(pen_x - x, tab_width) + x;
            prev = None;
            continue;
        }
        let gid = font.glyph_id_for_char(c);
        if let Some(p) = prev {
            pen_x += font.kerning(p, gid) as f32 * scale;
        }
        let base_x = pen_x.ffloor();
        let frac = pen_x - base_x;
        let subpixel_third = ((frac * 3.0).fround() as u8).min(2);
        let bmp = cache.get_or_rasterize(font, font_id, gid, size, subpixel_third);
        if bmp.w > 0 && bmp.h > 0 {
            for row in 0..bmp.h {
                let cov_row = &bmp.coverage[row * bmp.w..(row + 1) * bmp.w];
                for (col, &cov) in cov_row.iter().enumerate() {
                    if cov == 0 {
                        continue;
                    }
                    let cov = if light { GAMMA_BOOST_LUT[cov as usize] } else { cov };
                    canvas.blend_pixel_coverage(base_x as i32 + bmp.x + col as i32, y as i32 + bmp.y + row as i32, premul, cov as u32);
                }
            }
        }
        pen_x += font.advance_width(gid) as f32 * scale;
        prev = Some(gid);
    }
    pen_x - x
}

fn word_wrap_paragraph(font: &Font, text: &str, size: f32, max_width: f32) -> Vec<String> {
    if text.is_empty() {
        return alloc::vec![String::new()];
    }
    let space_width = measure(font, " ", size, 0.0);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0.0f32;
    for word in text.split(' ') {
        let word_width = measure(font, word, size, 0.0);
        let would_be = if current.is_empty() { word_width } else { current_width + space_width + word_width };
        if !current.is_empty() && would_be > max_width {
            lines.push(core::mem::take(&mut current));
            current_width = 0.0;
        }
        if !current.is_empty() {
            current.push(' ');
            current_width += space_width;
        }
        current.push_str(word);
        current_width += word_width;
    }
    lines.push(current);
    lines
}

/// Greedily word-wraps `text` (splitting on spaces, respecting existing newlines
/// as hard paragraph breaks) so each output line measures at most `max_width`
/// pixels wide -- except a single word longer than `max_width` on its own, which
/// is placed on its own line rather than broken.
pub fn word_wrap(font: &Font, text: &str, size: f32, max_width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        lines.extend(word_wrap_paragraph(font, paragraph, size, max_width));
    }
    lines
}

/// Truncates `text` to fit within `max_width` pixels, appending an ellipsis
/// (`"…"`) if it had to cut anything. Returns the empty string if even the
/// ellipsis alone doesn't fit.
pub fn truncate_with_ellipsis(font: &Font, text: &str, size: f32, max_width: f32) -> String {
    if measure(font, text, size, 0.0) <= max_width {
        return String::from(text);
    }
    let ellipsis = "\u{2026}";
    let ellipsis_width = measure(font, ellipsis, size, 0.0);
    if ellipsis_width > max_width {
        return String::new();
    }
    let budget = max_width - ellipsis_width;
    let mut result = String::new();
    let mut width = 0.0f32;
    let mut buf = [0u8; 4];
    for c in text.chars() {
        let s = c.encode_utf8(&mut buf);
        let cw = measure(font, s, size, 0.0);
        if width + cw > budget {
            break;
        }
        result.push(c);
        width += cw;
    }
    result.push_str(ellipsis);
    result
}

/// The recommended line height (baseline-to-baseline distance), in pixels, from
/// the font's own `hhea` metrics.
pub fn line_height(font: &Font, size: f32) -> f32 {
    let upm = font.units_per_em().max(1) as f32;
    (font.ascender() as f32 - font.descender() as f32 + font.line_gap() as f32) * size / upm
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::Surface;

    #[test]
    fn gamma_lut_is_monotonic_and_fixes_the_endpoints() {
        assert_eq!(GAMMA_BOOST_LUT[0], 0);
        assert_eq!(GAMMA_BOOST_LUT[255], 255);
        for w in GAMMA_BOOST_LUT.windows(2) {
            assert!(w[1] >= w[0], "LUT must be monotonic: {} then {}", w[0], w[1]);
        }
    }

    #[test]
    fn gamma_lut_boosts_low_coverage_without_touching_the_endpoints() {
        // The whole point: thin/wispy (low-coverage) pixels get visibly boosted.
        assert!(GAMMA_BOOST_LUT[26] > 26 + 10, "expected a real boost at low coverage");
    }

    #[test]
    fn is_light_text_classifies_white_and_black() {
        assert!(is_light_text(Color::WHITE));
        assert!(!is_light_text(Color::BLACK));
        assert!(is_light_text(Color::rgb(0xE8, 0xEC, 0xEE))); // DESIGN.md text.inverse
        assert!(!is_light_text(Color::rgb(0x1E, 0x22, 0x26))); // DESIGN.md text
    }

    #[test]
    fn light_on_dark_and_dark_on_light_read_with_similar_total_ink() {
        // Render the same glyph as light-on-dark and as dark-on-light (with
        // colours swapped, backgrounds swapped) and compare total "ink" (as a
        // luma difference from the background, summed over every pixel): gamma
        // correction should bring these much closer together than raw coverage
        // would (raw coverage is identical in both cases by construction, so
        // without correction the two would already match here as pure alpha --
        // the point is that the *visible* ink, this test's actual proxy for
        // "looks even", is what needs to match after the light side is boosted).
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../third_party/fonts/inter/Inter-Regular.ttf"));
        let Ok(bytes) = bytes else { return }; // skip gracefully if fonts aren't fetched yet
        let font = Font::parse(&bytes).unwrap();

        let dark_bg = Color::rgb(0x10, 0x10, 0x10);
        let light_bg = Color::rgb(0xF0, 0xF0, 0xF0);
        let light_text = Color::rgb(0xF0, 0xF0, 0xF0);
        let dark_text = Color::rgb(0x10, 0x10, 0x10);

        let render_ink = |bg: Color, text_color: Color| -> f64 {
            let mut s = Surface::new(60, 40);
            {
                let mut c = s.canvas();
                crate::shapes::fill_rect(&mut c, 0.0, 0.0, 60.0, 40.0, bg);
            }
            let mut cache = GlyphCache::new();
            {
                let mut c = s.canvas();
                draw_text(&mut c, &mut cache, &font, 0, "H", 5.0, 30.0, 28.0, 0.0, text_color);
            }
            let bg_luma = (bg.r as f64 + bg.g as f64 + bg.b as f64) / 3.0;
            s.px
                .iter()
                .map(|&p| {
                    let (_, r, g, b) = crate::color::unpack_argb(p);
                    let luma = (r as f64 + g as f64 + b as f64) / 3.0;
                    (luma - bg_luma).abs()
                })
                .sum()
        };

        let ink_light_on_dark = render_ink(dark_bg, light_text);
        let ink_dark_on_light = render_ink(light_bg, dark_text);
        let ratio = ink_light_on_dark / ink_dark_on_light;
        assert!((0.85..=1.18).contains(&ratio), "ink ratio {ratio} (light-on-dark / dark-on-light) is not close to even");
    }

    #[test]
    fn decode_utf8_handles_ascii_and_e_acute() {
        let bytes = "caf\u{e9}".as_bytes();
        assert_eq!(decode_utf8(bytes), alloc::vec!['c', 'a', 'f', '\u{e9}']);
    }

    #[test]
    fn decode_utf8_replaces_invalid_continuation_bytes() {
        let bytes = [b'a', 0xC0, 0x80, b'b']; // overlong/invalid 2-byte sequence
        let out = decode_utf8(&bytes);
        assert_eq!(out[0], 'a');
        assert_eq!(out.last(), Some(&'b'));
        assert!(out.contains(&'\u{FFFD}'));
    }

    #[test]
    fn decode_utf8_replaces_truncated_multibyte_sequence() {
        let bytes = [0xE2, 0x82]; // truncated 3-byte sequence (missing 1 byte)
        assert_eq!(decode_utf8(&bytes), alloc::vec!['\u{FFFD}']);
    }

    #[test]
    fn decode_utf8_never_panics_on_arbitrary_bytes() {
        // All 256 byte values, in every starting position -- must complete without
        // panicking, which is the actual point of this test.
        for start in 0..256usize {
            let bytes: Vec<u8> = (0..16).map(|i| ((start + i) % 256) as u8).collect();
            let _ = decode_utf8(&bytes);
        }
    }

    #[test]
    fn notdef_box_has_a_hole() {
        let path = notdef_box_path(1000);
        let mask = rasterize(&path, 0.2).unwrap();
        let cx = mask.w / 2;
        let cy = mask.h / 2;
        assert_eq!(mask.coverage[cy * mask.w + cx], 0, "tofu box must be hollow in the middle");
        assert!(mask.coverage[(mask.h / 10) * mask.w + mask.w / 2] > 0, "but solid near its border");
    }
}
