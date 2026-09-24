//! Framebuffer text console (brief M1-T7): a heap-allocated shadow buffer
//! of `cols x rows` cells plus a heap-allocated pixel back buffer, a
//! scrolling log, an ANSI colour subset, and an underline cursor that's
//! hidden while a write is in progress and redrawn once it's done.
//!
//! `Console<D: FbDevice>` never reads from the real framebuffer (MMIO
//! reads are slow, and Limine doesn't promise the memory is even
//! readable) -- every glyph and colour lives in `cells`/`back`, and only
//! the pixels that changed are ever blitted back out through `D`. This
//! also makes the type testable: `#[test_case]`s construct their own
//! `Console<FakeFb>` over an in-memory fake device (see
//! `test_cases::console`) instead of touching real hardware, and the
//! global console below (`init`/`feed`/`replay_boot_log`) is just one
//! particular `Console<framebuffer::Device>` behind a lock.
//!
//! Boot-time messages logged before `init` runs are captured by
//! `boot_ring` instead and replayed once it does (see that module and
//! `replay_boot_log`), so the on-screen log is complete rather than
//! starting partway through boot.

mod ansi;
pub mod boot_ring;
mod cell;
pub mod font;

use alloc::vec;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};

use ansi::{Action, Attrs, Parser};
use boot_ring::BootRing;
pub use cell::{Cell, Rgb};

use crate::framebuffer;
use crate::mm::addr::FRAME_SIZE;
use crate::mm::pmm;
use crate::sync::IrqMutex;
use font::Font;

/// Default scale for glyphs: 0 means auto-select based on screen width
/// (brief M2-T4). Explicit non-zero values override auto-selection.
const DEFAULT_SCALE: u64 = 0;

const PANIC_FG: Rgb = (0xff, 0x50, 0x50);

/// Devices `Console` can draw onto: implemented for the real Limine
/// framebuffer (`framebuffer::Device`) and, in tests, an in-memory fake
/// (`test_cases::console::FakeFb`) so unit tests never touch the real
/// screen.
pub trait FbDevice {
    fn width(&self) -> u64;
    fn height(&self) -> u64;

    /// Writes one contiguous row of already-decided colours starting at
    /// `(x0, y0)`. `Console` only ever calls this with `x0 + pixels.len()
    /// <= width()` and `y0 < height()`, but implementations should still
    /// bounds-check defensively -- the same stance `framebuffer::put_pixel`
    /// takes: an out-of-range coordinate is a silent no-op, never a panic
    /// or an out-of-bounds write.
    fn blit_row(&mut self, x0: u64, y0: u64, pixels: &[Rgb]);

    /// Fills the entire device with one solid colour, a row at a time via
    /// `blit_row`. Used once by `Console::new` (so a screen size that
    /// isn't an exact multiple of the cell size never shows a stale
    /// border of firmware/bootloader output) and by `ESC[2J` (so a clear
    /// reaches every physical pixel, not just the console's own grid).
    fn clear(&mut self, rgb: Rgb) {
        let row = vec![rgb; self.width() as usize];
        for y in 0..self.height() {
            self.blit_row(0, y, &row);
        }
    }
}

pub struct Console<D: FbDevice> {
    device: D,
    cols: usize,
    rows: usize,
    scale: usize,
    font: Font,
    /// The logical grid -- the "shadow buffer" (brief M1-T7 step 1),
    /// row-major, index `row * cols + col`. Holds exactly what was
    /// printed; the cursor's underline is a rendering overlay on top of
    /// this, never stored here.
    cells: Vec<Cell>,
    /// A full pixel mirror of the console's on-screen area (`cols *
    /// cell_w` by `rows * cell_h`), row-major. Scrolling memmoves this
    /// (and `cells`) instead of re-rendering every glyph from font data;
    /// every blit's source data comes from here.
    back: Vec<Rgb>,
    cursor_col: usize,
    cursor_row: usize,
    parser: Parser,
}

/// The `cols`/`rows`/buffers a successful allocation attempt in
/// `Console::try_new` produced, still separate from a real `Console`
/// (which also owns the device) so a failed attempt at one scale never
/// has to give back a partially-built `Console`.
struct Buffers {
    cols: usize,
    rows: usize,
    scale: usize,
    font: Font,
    cells: Vec<Cell>,
    back: Vec<Rgb>,
}

impl Buffers {
    /// Bytes `cols x rows` cells plus a `cols*cell_w x rows*cell_h` pixel
    /// back buffer would need, computed before anything is actually
    /// allocated (brief M1-T7 fix 5) so `try_new` can reject a shape that
    /// obviously won't fit without ever attempting the allocation.
    /// Saturating throughout: this only ever feeds a size comparison, so
    /// a hypothetical overflow must come out "too big" (and get rejected)
    /// rather than wrapping around into something that looks small.
    fn bytes_needed(cols: usize, rows: usize, cell_w: usize, cell_h: usize) -> usize {
        let cells = cols.saturating_mul(rows).saturating_mul(size_of::<Cell>());
        let back = cols
            .saturating_mul(cell_w)
            .saturating_mul(rows)
            .saturating_mul(cell_h)
            .saturating_mul(size_of::<Rgb>());
        cells.saturating_add(back)
    }

    /// Tries to allocate the shadow/back buffers for a `device`-sized
    /// console at `scale`. Refuses to even attempt it if `bytes_needed`
    /// exceeds `budget`; otherwise uses `try_reserve_exact` (never the
    /// panicking/aborting `vec!`/`Vec::with_capacity`) for the actual
    /// allocation, so an allocator-level failure (e.g. the PMM's free
    /// memory happens to be fragmented despite the size fitting `budget`)
    /// also comes back as `None` instead of aborting the kernel.
    fn try_alloc(device: &impl FbDevice, font: Font, scale: usize, budget: usize) -> Option<Self> {
        let cell_w = font.width() as usize * scale;
        let cell_h = font.height() as usize * scale;
        let cols = ((device.width() as usize) / cell_w).max(1);
        let rows = ((device.height() as usize) / cell_h).max(1);

        if Self::bytes_needed(cols, rows, cell_w, cell_h) > budget {
            return None;
        }

        let mut cells = Vec::new();
        cells.try_reserve_exact(cols * rows).ok()?;
        cells.resize(cols * rows, Cell::blank(ansi::DEFAULT_FG, ansi::DEFAULT_BG));

        let back_len = cols * cell_w * rows * cell_h;
        let mut back = Vec::new();
        back.try_reserve_exact(back_len).ok()?;
        back.resize(back_len, ansi::DEFAULT_BG);

        Some(Self { cols, rows, scale, font, cells, back })
    }
}

impl<D: FbDevice> Console<D> {
    /// Half of the PMM's *current* free memory, in bytes -- the ceiling
    /// `try_new` allocates the shadow/back buffers under (brief M1-T7 fix
    /// 5). A screen console is a nice-to-have; it must never be able to
    /// starve the rest of a booting kernel of memory just because the
    /// display happens to be very large.
    fn allocation_budget() -> usize {
        (pmm::stats().free / 2).saturating_mul(FRAME_SIZE)
    }

    /// Builds a console over `device`, preferring `scale` but falling
    /// back to scale 1 if `scale`'s shadow/back buffers would need more
    /// than half of the PMM's free memory (or the allocator still can't
    /// satisfy it despite fitting that budget) -- and giving up
    /// (returning `None`, `device` simply dropped) if even scale 1
    /// doesn't fit. Never aborts: see `Buffers::try_alloc`.
    pub fn try_new(mut device: D, scale: u64) -> Option<Self> {
        // Brief M2-T4: select font based on screen width if scale is auto (0)
        let (font, requested_scale) = if scale == 0 {
            Font::select_for_width(device.width())
        } else {
            (Font::Font8x16, scale.max(1))
        };

        let requested = requested_scale as usize;
        let budget = Self::allocation_budget();

        let buffers = if requested != 1 {
            Buffers::try_alloc(&device, font, requested, budget)
                .or_else(|| Buffers::try_alloc(&device, font, 1, budget))
        } else {
            Buffers::try_alloc(&device, font, 1, budget)
        }?;

        device.clear(ansi::DEFAULT_BG);
        let mut console = Self {
            device,
            cols: buffers.cols,
            rows: buffers.rows,
            scale: buffers.scale,
            font: buffers.font,
            cells: buffers.cells,
            back: buffers.back,
            cursor_col: 0,
            cursor_row: 0,
            parser: Parser::new(),
        };
        console.draw_cursor();
        Some(console)
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_col, self.cursor_row)
    }

    /// The cell at `(col, row)` as last printed -- never reflects the
    /// cursor overlay, since that's drawn straight into the pixel back
    /// buffer and never stored in `cells` (see the struct docs above).
    pub fn cell(&self, col: usize, row: usize) -> Cell {
        self.cells[row * self.cols + col]
    }

    /// Overrides the current draw attributes directly, bypassing SGR
    /// parsing (`panic_print`: the message is always bright red,
    /// regardless of whatever attributes were active when the panic
    /// happened).
    pub(crate) fn force_attrs(&mut self, attrs: Attrs) {
        self.parser.set_attrs(attrs);
    }

    /// Feeds every byte of `bytes` through the ANSI parser and the
    /// console's own control-character handling. The cursor's underline
    /// is erased before this starts and redrawn (at wherever the cursor
    /// ends up) only once every byte has been processed, so it never
    /// visibly flickers mid-write.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.hide_cursor();
        for &b in bytes {
            self.feed_byte(b);
        }
        self.draw_cursor();
    }

    fn attrs(&self) -> Attrs {
        self.parser.attrs()
    }

    fn cell_w(&self) -> usize {
        self.font.width() as usize * self.scale
    }

    fn cell_h(&self) -> usize {
        self.font.height() as usize * self.scale
    }

    fn back_width(&self) -> usize {
        self.cols * self.cell_w()
    }

    fn feed_byte(&mut self, byte: u8) {
        match self.parser.feed(byte) {
            Action::Emit(b) => self.put_visible_byte(b),
            // The parser already updated its own attributes; nothing
            // else to do until the next glyph is drawn with them.
            Action::SetAttrs => {}
            Action::ClearScreen => self.clear_screen(),
            Action::Home => {
                self.cursor_col = 0;
                self.cursor_row = 0;
            }
            Action::None => {}
        }
    }

    /// Everything the ANSI parser passed through unchanged: the control
    /// characters this console itself understands (brief M1-T7 step 2),
    /// or an ordinary glyph.
    fn put_visible_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.newline(),
            b'\r' => self.cursor_col = 0,
            b'\t' => self.tab(),
            0x08 | 0x7f => self.backspace(),
            _ => self.print_char(byte),
        }
    }

    fn print_char(&mut self, byte: u8) {
        let attrs = self.attrs();
        let idx = self.cursor_row * self.cols + self.cursor_col;
        self.cells[idx] = Cell { glyph: byte, fg: attrs.fg, bg: attrs.bg };
        self.redraw_cell(self.cursor_col, self.cursor_row);

        self.cursor_col += 1;
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.advance_row();
        }
    }

    fn newline(&mut self) {
        self.cursor_col = 0;
        self.advance_row();
    }

    fn advance_row(&mut self) {
        self.cursor_row += 1;
        if self.cursor_row >= self.rows {
            self.scroll();
            self.cursor_row = self.rows - 1;
        }
    }

    fn tab(&mut self) {
        const TAB_STOP: usize = 8;
        let next = (self.cursor_col / TAB_STOP + 1) * TAB_STOP;
        self.cursor_col = next.min(self.cols - 1);
    }

    fn backspace(&mut self) {
        if self.cursor_col == 0 {
            return;
        }
        self.cursor_col -= 1;
        let attrs = self.attrs();
        let idx = self.cursor_row * self.cols + self.cursor_col;
        self.cells[idx] = Cell::blank(attrs.fg, attrs.bg);
        self.redraw_cell(self.cursor_col, self.cursor_row);
    }

    fn clear_screen(&mut self) {
        let attrs = self.attrs();
        for c in self.cells.iter_mut() {
            *c = Cell::blank(attrs.fg, attrs.bg);
        }
        for px in self.back.iter_mut() {
            *px = attrs.bg;
        }
        self.device.clear(attrs.bg);
    }

    /// Moves every row up by one: memmoves `cells` and `back` (never
    /// re-rendering a single glyph from font data) and fills the newly
    /// vacated last row with blanks, then blits the whole refreshed
    /// `back` buffer out -- brief M1-T7 step 2's "scroll by one row".
    fn scroll(&mut self) {
        let attrs = self.attrs();
        let cell_h = self.cell_h();
        let bw = self.back_width();

        self.cells.copy_within(self.cols.., 0);
        for c in self.cells[(self.rows - 1) * self.cols..].iter_mut() {
            *c = Cell::blank(attrs.fg, attrs.bg);
        }

        let row_px = bw * cell_h;
        self.back.copy_within(row_px.., 0);
        let back_len = self.back.len();
        for px in self.back[back_len - row_px..].iter_mut() {
            *px = attrs.bg;
        }

        for y in 0..self.rows * cell_h {
            let off = y * bw;
            let (device, back) = (&mut self.device, &self.back);
            device.blit_row(0, y as u64, &back[off..off + bw]);
        }
    }

    /// Renders `cells[col, row]` into `back` from font data and blits
    /// just that cell's rectangle out -- the "dirty region" for a single
    /// character (brief M1-T7 step 2), as opposed to `scroll`'s whole-area
    /// blit.
    fn redraw_cell(&mut self, col: usize, row: usize) {
        let (cw, ch, scale, bw) = (self.cell_w(), self.cell_h(), self.scale, self.back_width());
        let cell = self.cells[row * self.cols + col];
        let glyph = self.font.glyph(cell.glyph);
        let font_w = self.font.width() as usize;
        let (x0, y0) = (col * cw, row * ch);

        for (gy, &bits) in glyph.iter().enumerate() {
            for sy in 0..scale {
                let row_off = (y0 + gy * scale + sy) * bw;
                for gx in 0..font_w {
                    let rgb = if bits & (1 << gx) != 0 { cell.fg } else { cell.bg };
                    for sx in 0..scale {
                        self.back[row_off + x0 + gx * scale + sx] = rgb;
                    }
                }
            }
        }

        for r in 0..ch {
            let off = (y0 + r) * bw;
            let (device, back) = (&mut self.device, &self.back);
            device.blit_row(x0 as u64, (y0 + r) as u64, &back[off + x0..off + x0 + cw]);
        }
    }

    /// Erases the cursor by simply redrawing the cell underneath it as
    /// plain text -- the underline is never part of `cells`, so this is
    /// enough to remove it.
    fn hide_cursor(&mut self) {
        self.redraw_cell(self.cursor_col, self.cursor_row);
    }

    /// Redraws the current cell, then overlays the bottom `scale` device
    /// rows with the current foreground colour to draw the underline.
    fn draw_cursor(&mut self) {
        self.redraw_cell(self.cursor_col, self.cursor_row);

        let (cw, ch, bw) = (self.cell_w(), self.cell_h(), self.back_width());
        let (x0, y0) = (self.cursor_col * cw, self.cursor_row * ch);
        let underline_h = self.scale.max(1);
        let fg = self.attrs().fg;

        for r in (ch - underline_h)..ch {
            let off = (y0 + r) * bw;
            for x in 0..cw {
                self.back[off + x0 + x] = fg;
            }
        }
        for r in (ch - underline_h)..ch {
            let off = (y0 + r) * bw;
            let (device, back) = (&mut self.device, &self.back);
            device.blit_row(x0 as u64, (y0 + r) as u64, &back[off + x0..off + x0 + cw]);
        }
    }
}

impl<D: FbDevice> fmt::Write for Console<D> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_bytes(s.as_bytes());
        Ok(())
    }
}

// --- The global console, wired into `kprint!`/`kprintln!` -------------------

static SCALE_PREF: AtomicU64 = AtomicU64::new(DEFAULT_SCALE);
static CONSOLE: IrqMutex<Option<Console<framebuffer::Device>>> = IrqMutex::new(None);
static BOOT_RING: IrqMutex<BootRing<{ boot_ring::CAPACITY }>> = IrqMutex::new(BootRing::new());

/// Sets the glyph scale a future `init` call constructs the console with
/// (default 0 = auto-select based on screen width). Has no effect on a
/// console that already exists; call it before `init` to change it.
pub fn set_scale(scale: u64) {
    SCALE_PREF.store(scale, Ordering::Relaxed);
}

/// Brings the global console up on `device`, if `Console::try_new` can
/// find a scale whose buffers fit in memory (see its own docs) --
/// otherwise logs `[console] disabled: insufficient memory` and leaves
/// the console unset, so `feed` keeps going to serial (and `BOOT_RING`)
/// only, same as before this ever ran.
///
/// Nothing already buffered by `feed` is shown until `replay_boot_log`
/// runs next -- kept as a separate step (rather than folded into this
/// one) specifically so `main.rs` can call it *before* printing its own
/// title/memory lines, not after. The backlog from `init()`/
/// `start_interrupts()` (ACPI, the PMM's own memory-map dump, ...) is
/// comfortably taller than one screen on a typical resolution, so
/// replaying it first and printing the banner last is what keeps the
/// banner on screen at the moment `gmake shot` captures it, instead of
/// the banner scrolling away under everything replayed after it.
pub fn init(device: framebuffer::Device) {
    let scale = SCALE_PREF.load(Ordering::Relaxed);
    match Console::try_new(device, scale) {
        Some(console) => *CONSOLE.lock() = Some(console),
        None => crate::kprintln!("[console] disabled: insufficient memory"),
    }
}

/// Replays whatever `feed` buffered into `BOOT_RING` before `init` ran,
/// in the order it was originally logged. A no-op if `init` was never
/// called (or failed, see its docs), or if nothing was buffered.
///
/// Copies the backlog out of `BOOT_RING` under its own lock only, then
/// feeds it to the console in `REPLAY_CHUNK`-sized pieces, taking (and
/// dropping) `CONSOLE`'s lock once per chunk rather than once for the
/// entire backlog (kernel-review fix 1): a boot log can run to several KiB,
/// and `IrqMutex::lock` disables interrupts for as long as it's held, so
/// redrawing all of it under one `CONSOLE` lock would starve the timer and
/// keyboard IRQs for that whole stretch. Dropping the lock between chunks
/// gives them a chance to run in between instead.
pub fn replay_boot_log() {
    let mut backlog = Vec::new();
    BOOT_RING.lock().for_each_segment(|segment| backlog.extend_from_slice(segment));

    const REPLAY_CHUNK: usize = 256;
    for chunk in backlog.chunks(REPLAY_CHUNK) {
        let mut slot = CONSOLE.lock();
        let Some(console) = slot.as_mut() else { return };
        console.write_bytes(chunk);
    }
}

/// A `fmt::Write` adapter that appends straight into `BOOT_RING`, so
/// `feed` can reuse `write_fmt` instead of formatting into an
/// intermediate buffer of its own.
struct RingWriter;

impl fmt::Write for RingWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        BOOT_RING.lock().write(s.as_bytes());
        Ok(())
    }
}

/// Called from `serial::_print` for every `kprint!`/`kprintln!`: once the
/// console exists, writes straight through to it; before that, buffers
/// into `BOOT_RING` instead (brief M1-T7 step 3). Never touched from IRQ
/// context (nothing in this kernel logs from one -- see the module docs
/// on `sync::IrqMutex` and `drivers::ps2::mod::irq_handler`), so taking
/// `CONSOLE`'s lock here can never race a handler that also wants it.
pub fn feed(args: fmt::Arguments) {
    let mut slot = CONSOLE.lock();
    if let Some(console) = slot.as_mut() {
        let _ = console.write_fmt(args);
        return;
    }
    drop(slot);
    let _ = RingWriter.write_fmt(args);
}

/// Called only from the top-level panic handler (DECISIONS.md D14):
/// `kprintln_emergency!` (lock-free, straight to the UART) already ran
/// first and unconditionally, so serial always has the message regardless
/// of what this does. This is a best-effort bonus for whoever's watching
/// the screen: `try_lock`, never `lock`, because the panicking code could
/// well be `Console::write_fmt`/`redraw_cell` itself (a bug in the
/// console's own code, or in something it calls), in which case
/// `CONSOLE`'s lock is already held on this same core and `lock()` would
/// spin forever. If the lock isn't free, this simply prints nothing to
/// the screen -- exactly like a panic while `SERIAL1` is already held
/// falls back to `serial::EmergencyWriter` instead of deadlocking on
/// `SERIAL1` itself.
pub fn panic_print(info: &core::panic::PanicInfo) {
    let Some(mut slot) = CONSOLE.try_lock() else { return };
    let Some(console) = slot.as_mut() else { return };

    console.force_attrs(Attrs { fg: PANIC_FG, bg: ansi::DEFAULT_BG, bold: true });
    let _ = match info.location() {
        Some(loc) => writeln!(console, "\nPANIC at {}:{}: {}", loc.file(), loc.line(), info.message()),
        None => writeln!(console, "\nPANIC at <unknown location>: {}", info.message()),
    };
}
