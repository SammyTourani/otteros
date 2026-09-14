//! A small fixed-capacity byte ring (brief M1-T7 step 3): everything
//! `kprint!`/`kprintln!` write before `console::init` has run gets
//! captured here instead, in order, so it can be replayed onto the
//! console the moment it exists -- the on-screen log should look
//! complete, not start partway through boot.
//!
//! `const fn new()` and no heap use whatsoever (`BootRing` is a plain
//! `[u8; CAP]`, not a `Vec`) deliberately: the very first `kprintln!`
//! calls (`[gdt] loaded`, the PMM's memory-map dump, ...) happen before
//! `mm::heap::init()` has run, so anything this needs has to work with no
//! allocator at all yet.

/// Bytes logged before the console exists, on a real boot, comfortably
/// fit in a few KiB (~2.4 KiB measured for everything from `[gdt]
/// loaded` through the keyboard's IRQ registration); 16 KiB leaves ample
/// headroom for a larger memory map or extra log lines without ever
/// growing -- and if it's ever exceeded anyway, `write` below drops the
/// *oldest* bytes rather than panicking or blocking.
pub const CAPACITY: usize = 16 * 1024;

pub struct BootRing<const CAP: usize> {
    buf: [u8; CAP],
    /// Index of the next byte `write` will fill.
    pos: usize,
    /// Whether `pos` has ever wrapped back to 0, i.e. whether `buf` holds
    /// `CAP` valid bytes (oldest-first starting at `pos`) rather than
    /// just the first `pos` of them.
    wrapped: bool,
}

impl<const CAP: usize> BootRing<CAP> {
    pub const fn new() -> Self {
        Self { buf: [0; CAP], pos: 0, wrapped: false }
    }

    /// Appends `bytes`, oldest-write-wins on overflow: once full, each
    /// new byte overwrites the oldest one still held, so `replay` always
    /// hands back the most recent `CAP` bytes, in the order they were
    /// written.
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.buf[self.pos] = b;
            self.pos += 1;
            if self.pos == CAP {
                self.pos = 0;
                self.wrapped = true;
            }
        }
    }

    /// Calls `f` with every byte written so far, oldest first, as one or
    /// two contiguous slices (two only if the ring has wrapped: the
    /// tail-end segment written before wrapping, then the head-end
    /// segment written after). Never more than two calls, so a caller
    /// that itself has per-call overhead (`Console::write_bytes` hides
    /// and redraws the cursor once per call) doesn't pay it once per
    /// byte.
    pub fn for_each_segment(&self, mut f: impl FnMut(&[u8])) {
        if self.wrapped {
            f(&self.buf[self.pos..]);
        }
        f(&self.buf[..self.pos]);
    }
}

impl<const CAP: usize> Default for BootRing<CAP> {
    fn default() -> Self {
        Self::new()
    }
}
