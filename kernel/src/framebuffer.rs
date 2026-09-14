//! Framebuffer device layer: Limine hands us a linear pixel buffer (see
//! `get()`); everything here is just bounds-checked pixel access plus
//! `Device`, the `console::FbDevice` implementation `console::init` draws
//! onto. All text rendering, colour, and scrolling now live in `console`
//! (brief M1-T7) -- this module doesn't know what a glyph is.

use limine::framebuffer::Framebuffer;
use limine::request::FramebufferRequest;

use crate::console::{FbDevice, Rgb};

#[used]
#[unsafe(link_section = ".requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

/// Returns the first framebuffer Limine gave us, if any.
pub fn get() -> Option<&'static Framebuffer> {
    FRAMEBUFFER_REQUEST.response()?.framebuffers().first().copied()
}

/// Packs 8-bit colour components using *this framebuffer's* actual channel
/// layout (Limine doesn't promise a fixed one, e.g. RGB vs BGR).
fn pack(fb: &Framebuffer, r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << fb.red_mask_shift) | (u32::from(g) << fb.green_mask_shift) | (u32::from(b) << fb.blue_mask_shift)
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
    // `fb.height * fb.pitch`-byte buffer `as_slice_mut` exposes. Every
    // write to this framebuffer goes through `console::CONSOLE`'s
    // `IrqMutex` (or, at boot, this same single-threaded call before
    // interrupts are enabled), so there is no concurrent access to race
    // with.
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

/// The real hardware backend for `console::Console`: wraps whichever
/// framebuffer Limine handed us and implements `console::FbDevice` in
/// terms of `pack`/`put_pixel` above, so `console` itself never needs to
/// know this framebuffer's actual pixel format (RGB vs BGR, 24 vs 32 bpp,
/// row pitch, ...).
pub struct Device {
    fb: &'static Framebuffer,
}

impl Device {
    pub fn new(fb: &'static Framebuffer) -> Self {
        Self { fb }
    }
}

// SAFETY: `Framebuffer` (from the `limine` crate) holds a raw `*mut ()`
// pointer to its pixel data, so it isn't `Sync` and `&'static Framebuffer`
// isn't `Send` by default -- but `Device` only ever uses it to compute
// bounds-checked byte offsets (`put_pixel`/`pack`, both plain arithmetic
// plus a bounds-checked slice write) and never stashes any thread-local
// state of its own. This kernel is single-core (DECISIONS.md D12: SMP
// scheduling is out of scope for now) and every access to a `Device`
// already goes through `console::CONSOLE`'s `IrqMutex`, so nothing ever
// touches one from two contexts at once regardless of this assertion.
unsafe impl Send for Device {}

impl FbDevice for Device {
    fn width(&self) -> u64 {
        self.fb.width
    }

    fn height(&self) -> u64 {
        self.fb.height
    }

    fn blit_row(&mut self, x0: u64, y0: u64, pixels: &[Rgb]) {
        for (i, &(r, g, b)) in pixels.iter().enumerate() {
            put_pixel(self.fb, x0 + i as u64, y0, pack(self.fb, r, g, b));
        }
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
