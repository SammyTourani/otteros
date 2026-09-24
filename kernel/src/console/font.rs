//! Font selection and glyph lookup (brief M2-T4).

use crate::{font8x16, font8x8};

/// Available console fonts.
#[derive(Clone, Copy, Debug)]
pub enum Font {
    Font8x8,
    Font8x16,
}

impl Font {
    /// Returns the width of glyphs in this font in pixels.
    pub fn width(&self) -> u64 {
        match self {
            Self::Font8x8 => font8x8::GLYPH_WIDTH,
            Self::Font8x16 => font8x16::GLYPH_WIDTH,
        }
    }

    /// Returns the height of glyphs in this font in pixels.
    pub fn height(&self) -> u64 {
        match self {
            Self::Font8x8 => font8x8::GLYPH_HEIGHT,
            Self::Font8x16 => font8x16::GLYPH_HEIGHT,
        }
    }

    /// Returns the bitmap for the given character in this font.
    /// The bitmap is always 16 bytes; for 8x8 fonts, each byte is padded to
    /// 16 rows (with the upper 8 rows blank).
    pub fn glyph(&self, ch: u8) -> [u8; 16] {
        match self {
            Self::Font8x8 => {
                // Pad 8x8 glyph to 16 bytes
                let glyph = font8x8::glyph(ch);
                let mut padded = [0u8; 16];
                // Place the 8x8 glyph in the lower half
                padded[4..12].copy_from_slice(&glyph);
                padded
            }
            Self::Font8x16 => font8x16::glyph(ch),
        }
    }

    /// Selects a font based on screen width. Brief M2-T4: 8x16 at scale 1
    /// on screens < 1600 px wide, otherwise 8x16 at scale 2 / 8x8 at scale 2.
    pub fn select_for_width(width: u64) -> (Self, u64) {
        if width < 1600 {
            (Self::Font8x16, 1)
        } else {
            (Self::Font8x16, 2)
        }
    }
}
