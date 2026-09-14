//! One character cell in `Console`'s shadow buffer: a glyph byte plus the
//! foreground/background colour it was drawn with. Deliberately holds no
//! cursor state -- the cursor's on-screen underline is a pure rendering
//! overlay (`Console::draw_cursor`), never part of a cell's own data, so
//! `Console::cell` always reflects exactly what was *printed*, regardless
//! of where the cursor happens to be sitting when a test (or anything
//! else) reads it back.

/// 8-bit-per-channel colour, in `(r, g, b)` order. Never packed into a
/// device's native pixel format until the very last step
/// (`FbDevice::blit_row`'s implementation) -- everything upstream of that
/// (cells, the pixel back buffer, the ANSI palette) stays in this
/// device-independent form.
pub type Rgb = (u8, u8, u8);

/// One `cols x rows` grid entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub glyph: u8,
    pub fg: Rgb,
    pub bg: Rgb,
}

impl Cell {
    /// A blank (space) cell in `fg`-on-`bg`, used to fill newly scrolled-in
    /// rows and to erase a cell on backspace/clear.
    pub const fn blank(fg: Rgb, bg: Rgb) -> Self {
        Self { glyph: b' ', fg, bg }
    }
}
