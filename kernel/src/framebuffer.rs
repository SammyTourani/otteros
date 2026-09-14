//! Framebuffer output: Limine hands us a linear pixel buffer, which we clear
//! to a dark colour and draw text onto with the embedded 8x8 font (see
//! `font8x8`). No scrolling console yet -- that's M1's "framebuffer console"
//! item in PLAN.md; M0 only needs a one-shot boot banner.

use core::fmt;

use limine::framebuffer::Framebuffer;
use limine::request::FramebufferRequest;

use crate::font8x8;

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

const BG: (u8, u8, u8) = (0x10, 0x14, 0x1c);
const FG: (u8, u8, u8) = (0xe0, 0xe0, 0xe0);
const FG_DIM: (u8, u8, u8) = (0x90, 0x98, 0xa0);

/// Returns the first framebuffer Limine gave us, if any.
pub fn get() -> Option<&'static Framebuffer> {
    FRAMEBUFFER_REQUEST.response()?.framebuffers().first().copied()
}

/// Packs 8-bit colour components using *this framebuffer's* actual channel
/// layout (Limine doesn't promise a fixed one, e.g. RGB vs BGR).
fn pack(fb: &Framebuffer, r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << fb.red_mask_shift)
        | (u32::from(g) << fb.green_mask_shift)
        | (u32::from(b) << fb.blue_mask_shift)
}

fn pixel_offset(fb: &Framebuffer, x: u64, y: u64) -> Option<usize> {
    if x >= fb.width || y >= fb.height {
        return None;
    }
    let bpp_bytes = u64::from(fb.bpp) / 8;
    Some((y * fb.pitch + x * bpp_bytes) as usize)
}

/// Writes one packed pixel value. Out-of-bounds coordinates are silently
/// dropped rather than panicking or touching memory outside the buffer --
/// this is exactly what the `framebuffer_bounds` test checks.
fn put_pixel(fb: &Framebuffer, x: u64, y: u64, color: u32) {
    let Some(offset) = pixel_offset(fb, x, y) else {
        return;
    };
    let bpp_bytes = usize::from(fb.bpp) / 8;
    // SAFETY: `pixel_offset` only returns `Some` when `x < fb.width` and
    // `y < fb.height`, so `offset .. offset + bpp_bytes` lies inside the
    // `fb.height * fb.pitch`-byte buffer `as_slice_mut` exposes. Nothing
    // else touches the framebuffer yet (single-threaded, interrupts not
    // enabled until M1), so there is no concurrent access to race with.
    unsafe {
        let bytes = fb.as_slice_mut();
        for i in 0..bpp_bytes {
            bytes[offset + i] = ((color >> (8 * i)) & 0xFF) as u8;
        }
    }
}

fn read_pixel(fb: &Framebuffer, x: u64, y: u64) -> Option<u32> {
    let offset = pixel_offset(fb, x, y)?;
    let bpp_bytes = usize::from(fb.bpp) / 8;
    // SAFETY: same bounds argument as `put_pixel`.
    unsafe {
        let bytes = fb.as_slice_mut();
        let mut value = 0u32;
        for i in 0..bpp_bytes.min(4) {
            value |= u32::from(bytes[offset + i]) << (8 * i);
        }
        Some(value)
    }
}

pub fn clear(fb: &Framebuffer, r: u8, g: u8, b: u8) {
    let color = pack(fb, r, g, b);
    for y in 0..fb.height {
        for x in 0..fb.width {
            put_pixel(fb, x, y, color);
        }
    }
}

/// Fills a `w x h` rectangle at `(x0, y0)` with a solid `(r, g, b)`
/// colour (brief M1-T6: `main.rs`'s typing echo uses this to blank the
/// previous line before redrawing it, rather than clearing -- and
/// redrawing the whole banner over -- the entire screen on every
/// keystroke). Out-of-bounds coordinates are silently dropped, same as
/// `put_pixel`. Colour is one `(u8, u8, u8)` tuple, not three separate
/// arguments, purely to stay under clippy's default argument-count limit.
pub fn clear_rect(fb: &Framebuffer, x0: u64, y0: u64, w: u64, h: u64, color: (u8, u8, u8)) {
    let packed = pack(fb, color.0, color.1, color.2);
    for y in y0..y0.saturating_add(h) {
        for x in x0..x0.saturating_add(w) {
            put_pixel(fb, x, y, packed);
        }
    }
}

fn draw_glyph(fb: &Framebuffer, x0: u64, y0: u64, ch: u8, color: u32) {
    let rows = font8x8::glyph(ch);
    for (row, bits) in rows.iter().enumerate() {
        for col in 0..font8x8::GLYPH_WIDTH {
            if bits & (1 << col) != 0 {
                put_pixel(fb, x0 + col, y0 + row as u64, color);
            }
        }
    }
}

pub fn draw_str(fb: &Framebuffer, x0: u64, y0: u64, s: &str, r: u8, g: u8, b: u8) {
    let color = pack(fb, r, g, b);
    let mut x = x0;
    for byte in s.bytes() {
        draw_glyph(fb, x, y0, byte, color);
        x += font8x8::GLYPH_WIDTH;
    }
}

/// Formats into a fixed, on-stack byte buffer -- we have no heap allocator
/// yet (that's M1), so `alloc::format!` isn't available.
struct FixedBuf<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl fmt::Write for FixedBuf<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let end = self.pos + bytes.len();
        if end > self.buf.len() {
            return Err(fmt::Error);
        }
        self.buf[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }
}

/// Clears the screen and draws the M0 boot banner plus the resolution.
pub fn draw_banner(fb: &Framebuffer) {
    use fmt::Write as _;

    clear(fb, BG.0, BG.1, BG.2);
    draw_str(fb, 16, 16, "OTTEROS  M0  HELLO FROM RING 0", FG.0, FG.1, FG.2);

    let mut raw = [0u8; 32];
    let len = {
        let mut w = FixedBuf { buf: &mut raw, pos: 0 };
        if write!(w, "{}X{} @ {}BPP", fb.width, fb.height, fb.bpp).is_ok() {
            w.pos
        } else {
            0
        }
    };
    if let Ok(text) = core::str::from_utf8(&raw[..len]) {
        draw_str(fb, 16, 32, text, FG_DIM.0, FG_DIM.1, FG_DIM.2);
    }
}

/// Draws a "<total> MiB total, <free> MiB free" line below `draw_banner`'s
/// resolution line (brief M1-T2: the PMM's boot summary, on the
/// framebuffer as well as serial). Takes plain MiB counts rather than an
/// `mm` type so this module stays independent of `mm` -- the caller (M1's
/// `main.rs`) does the frames-to-MiB conversion.
pub fn draw_mem_line(fb: &Framebuffer, total_mib: u64, free_mib: u64) {
    use fmt::Write as _;

    let mut raw = [0u8; 48];
    let len = {
        let mut w = FixedBuf { buf: &mut raw, pos: 0 };
        if write!(w, "MEM: {total_mib} MiB total, {free_mib} MiB free").is_ok() {
            w.pos
        } else {
            0
        }
    };
    if let Ok(text) = core::str::from_utf8(&raw[..len]) {
        draw_str(fb, 16, 48, text, FG_DIM.0, FG_DIM.1, FG_DIM.2);
    }
}

/// Writes a known pixel, attempts a couple of wildly out-of-bounds writes,
/// then confirms the known pixel survived unharmed. Exercised by a
/// `#[test_case]` in `test_main.rs`.
pub fn bounds_self_test() -> bool {
    let Some(fb) = get() else { return false };
    if fb.width == 0 || fb.height == 0 {
        return false;
    }

    let probe = pack(fb, 0xab, 0xcd, 0xef);
    put_pixel(fb, 0, 0, probe);

    // These must be no-ops, not panics and not out-of-bounds writes.
    put_pixel(fb, fb.width + 10_000, fb.height + 10_000, 0x00ff_ffff);
    put_pixel(fb, fb.width, fb.height, 0x00ff_ffff); // exactly-at-edge is still OOB
    put_pixel(fb, u64::MAX, 0, 0x00ff_ffff);

    read_pixel(fb, 0, 0) == Some(probe)
}
