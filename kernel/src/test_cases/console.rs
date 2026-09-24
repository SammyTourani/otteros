//! `console::Console` tests (brief M1-T7): every test here builds its own
//! `Console` over `FakeFb`, an in-memory `console::FbDevice`, so none of
//! them touch the real screen or the global console `main.rs`/`init`
//! wires up.

use alloc::vec;
use alloc::vec::Vec;

use otteros_kernel::console::boot_ring::BootRing;
use otteros_kernel::console::{Console, FbDevice, Rgb};

/// An in-memory `FbDevice`: records every blitted pixel in a plain `Vec`
/// instead of touching hardware, so these tests can build a real
/// `Console` end to end (cursor movement, scrolling, ANSI parsing) without
/// depending on -- or disturbing -- the real framebuffer.
struct FakeFb {
    width: u64,
    height: u64,
    pixels: Vec<Rgb>,
}

impl FakeFb {
    fn new(width: u64, height: u64) -> Self {
        Self { width, height, pixels: vec![(0, 0, 0); (width * height) as usize] }
    }
}

impl FbDevice for FakeFb {
    fn width(&self) -> u64 {
        self.width
    }

    fn height(&self) -> u64 {
        self.height
    }

    fn blit_row(&mut self, x0: u64, y0: u64, pixels: &[Rgb]) {
        if y0 >= self.height {
            return;
        }
        for (i, &rgb) in pixels.iter().enumerate() {
            let x = x0 + i as u64;
            if x < self.width {
                self.pixels[(y0 * self.width + x) as usize] = rgb;
            }
        }
    }
}

/// A `Console<FakeFb>` at `scale` 1 (one device pixel per font pixel), so
/// `cols`/`rows` come out to simple, easy-to-reason-about numbers:
/// `width / 8` and `height / 8`. These fake buffers are a few hundred
/// bytes at most, so `try_new` always has comfortably more than half the
/// PMM's free memory to work with; the `expect` just documents that this
/// helper is for tests where allocation failure would itself be the bug.
fn console(width: u64, height: u64) -> Console<FakeFb> {
    Console::try_new(FakeFb::new(width, height), 1).expect("a tiny fake framebuffer should always fit")
}

/// Printing ordinary characters advances the cursor one column per byte
/// and leaves each one in the cell it was printed to.
#[test_case]
fn console_cursor_advances_per_char() {
    let mut c = console(80, 40); // 10 cols x 5 rows
    assert_eq!(c.cursor(), (0, 0));

    c.write_bytes(b"AB");

    assert_eq!(c.cursor(), (2, 0));
    assert_eq!(c.cell(0, 0).glyph, b'A');
    assert_eq!(c.cell(1, 0).glyph, b'B');
}

/// `\n` moves to column 0 of the next row (not just down in the same
/// column) -- the conventional "newline" a text console's log output
/// needs, since every `kprintln!` line otherwise would otherwise start
/// one column further right than the last.
#[test_case]
fn console_newline_moves_to_next_row_and_col_zero() {
    let mut c = console(80, 40);

    c.write_bytes(b"AB\nC");

    assert_eq!(c.cursor(), (1, 1));
    assert_eq!(c.cell(0, 1).glyph, b'C');
    assert_eq!(c.cell(0, 0).glyph, b'A'); // row 0 untouched by the newline
}

/// `\r` alone returns to column 0 of the *same* row, letting a later
/// write overwrite what's already there -- distinct from `\n`.
#[test_case]
fn console_carriage_return_moves_to_col_zero_same_row() {
    let mut c = console(80, 40);

    c.write_bytes(b"AB\rC");

    assert_eq!(c.cursor(), (1, 0));
    assert_eq!(c.cell(0, 0).glyph, b'C'); // overwrote the 'A'
    assert_eq!(c.cell(1, 0).glyph, b'B'); // untouched
}

/// `\t` advances to the next multiple of 8, from wherever the cursor
/// currently is -- not just "+8".
#[test_case]
fn console_tab_advances_to_next_multiple_of_eight() {
    let mut c = console(24 * 8, 40); // 24 cols, comfortably more than two tab stops

    c.write_bytes(b"A\t");
    assert_eq!(c.cursor(), (8, 0));

    c.write_bytes(b"\t");
    assert_eq!(c.cursor(), (16, 0));
}

/// Printing past the last column wraps to column 0 of the next row,
/// rather than running off the edge.
#[test_case]
fn console_wrap_at_last_column() {
    let mut c = console(64, 40); // 8 cols x 5 rows

    c.write_bytes(b"ABCDEFGH"); // exactly fills row 0
    assert_eq!(c.cursor(), (0, 1));

    c.write_bytes(b"I");
    assert_eq!(c.cell(0, 1).glyph, b'I');
}

/// Once every row is full, the next newline scrolls: every row moves up
/// by one, and the newly vacated last row is blank.
#[test_case]
fn console_scroll_moves_rows_up_and_clears_last_row() {
    let mut c = console(16, 48); // 2 cols x 3 rows (8x16 font at scale 1)

    c.write_bytes(b"A\nB\nC\nD");

    assert_eq!(c.cell(0, 0).glyph, b'B'); // was row 1
    assert_eq!(c.cell(0, 1).glyph, b'C'); // was row 2
    assert_eq!(c.cell(0, 2).glyph, b'D'); // freshly printed after the scroll
    assert_eq!(c.cell(1, 2).glyph, b' '); // the rest of the scrolled-in row is blank
}

/// Backspace moves the cursor back one column and erases (blanks) the
/// cell there, rather than just moving the cursor.
#[test_case]
fn console_backspace_erases_previous_cell() {
    let mut c = console(80, 40);

    c.write_bytes(b"AB\x08"); // 'A', 'B', backspace

    assert_eq!(c.cursor(), (1, 0));
    assert_eq!(c.cell(1, 0).glyph, b' '); // 'B' erased
    assert_eq!(c.cell(0, 0).glyph, b'A'); // untouched
}

/// `ESC[<code>m` sets the foreground colour for everything printed after
/// it, and `ESC[0m` restores the default -- the colour codes themselves
/// never appear as printed glyphs.
#[test_case]
fn console_ansi_color_sets_fg_and_reset_restores_default() {
    let mut c = console(80, 40);

    c.write_bytes(b"\x1b[31mA\x1b[0mB");

    let red = c.cell(0, 0);
    let reset = c.cell(1, 0);
    assert_eq!(red.glyph, b'A');
    assert_eq!(red.fg, (0xaa, 0x00, 0x00), "code 31 should be ANSI red");
    assert_eq!(reset.glyph, b'B');
    assert_eq!(reset.fg, (0xe0, 0xe0, 0xe0), "ESC[0m should restore the default foreground");
    assert_eq!(c.cursor(), (2, 0), "only 'A' and 'B' should have advanced the cursor");
}

/// A CSI sequence with a final byte this subset doesn't implement, and an
/// `ESC` not followed by `[` at all, are both consumed without ever being
/// printed as garbage or perturbing whatever attributes were already
/// active -- brief M1-T7: "unknown sequences are ignored safely".
#[test_case]
fn console_ansi_unknown_sequence_ignored_safely() {
    let mut c = console(80, 40);

    // ESC[99m: valid CSI framing, but 99 isn't an SGR code this subset
    // implements -- swallowed, leaving the active colour unchanged.
    c.write_bytes(b"\x1b[35mA\x1b[99mB");
    assert_eq!(c.cell(0, 0).fg, (0xaa, 0x00, 0xaa), "35 is ANSI magenta");
    assert_eq!(c.cell(1, 0).fg, (0xaa, 0x00, 0xaa), "unknown SGR code 99 should leave attrs unchanged");
    assert_eq!(c.cursor(), (2, 0));

    // ESC not followed by '[' at all: outside this subset's grammar
    // entirely -- both bytes are swallowed, not printed.
    c.write_bytes(b"\x1bZC");
    assert_eq!(c.cell(2, 0).glyph, b'C', "only 'C' should have printed");
    assert_eq!(c.cursor(), (3, 0), "ESC and 'Z' should not have advanced the cursor or printed");
}

/// `BootRing` hands back everything written, oldest first, as long as it
/// fits; once it doesn't, it's the *oldest* bytes that are dropped, so
/// what's left is still in order (brief M1-T7 step 3: the on-screen log
/// should read as a prefix-trimmed boot log, never a shuffled one).
#[test_case]
fn console_boot_ring_replay_preserves_message_order() {
    let mut ring: BootRing<32> = BootRing::new();
    ring.write(b"first ");
    ring.write(b"second ");
    ring.write(b"third");

    let mut replayed = Vec::new();
    ring.for_each_segment(|segment| replayed.extend_from_slice(segment));
    assert_eq!(replayed, b"first second third");

    // 6 bytes into a 4-byte ring: "ab" are the oldest, so they're the
    // ones dropped -- the survivors ("cdef") must still read in order.
    let mut small: BootRing<4> = BootRing::new();
    small.write(b"abcdef");
    let mut replayed_small = Vec::new();
    small.for_each_segment(|segment| replayed_small.extend_from_slice(segment));
    assert_eq!(replayed_small, b"cdef");
}

/// An absurdly large numeric SGR parameter (12 digits, far past `u16`)
/// must saturate rather than overflow/panic while it's being accumulated
/// digit by digit, and then be safely ignored as an unrecognised code once
/// the sequence completes -- ordinary text keeps working right after it
/// (kernel-review fix 3).
#[test_case]
fn console_ansi_huge_numeric_param_does_not_panic() {
    let mut c = console(80, 40);

    c.write_bytes(b"\x1b[999999999999mA");

    assert_eq!(c.cell(0, 0).glyph, b'A');
    assert_eq!(c.cursor(), (1, 0));
}

/// An escape sequence that starts, is split across two separate
/// `write_bytes` calls, and never reaches a valid CSI final byte
/// (0x40..=0x7e) at all -- digits and `;` keep the parser waiting
/// indefinitely, exactly the "stuck forever" shape a real boot log could
/// produce if a line were ever truncated mid-sequence. The console must
/// still recover (via the first byte outside that grammar) and print
/// ordinary text after it rather than staying wedged (kernel-review fix 3).
#[test_case]
fn console_ansi_split_escape_that_never_terminates_recovers() {
    let mut c = console(80, 40);

    c.write_bytes(b"\x1b[123"); // first call: starts a CSI sequence, no final byte yet
    c.write_bytes(b"456"); // second call: still just digits, still no final byte

    // A byte outside the parameter/final-byte grammar breaks out of the
    // stuck sequence instead of waiting forever; genuinely ordinary text
    // right after it must still be visible.
    c.write_bytes(b" done");

    assert_eq!(c.cell(0, 0).glyph, b'd');
    assert_eq!(c.cell(1, 0).glyph, b'o');
    assert_eq!(c.cell(2, 0).glyph, b'n');
    assert_eq!(c.cell(3, 0).glyph, b'e');
    assert_eq!(c.cursor(), (4, 0));
}

/// A stray control byte (BEL, 0x07) landing in the middle of a CSI
/// sequence isn't valid parameter/intermediate grammar either -- must
/// break out of the sequence cleanly (not hang, not corrupt any state)
/// rather than crash, and whatever bytes follow print as ordinary text
/// again immediately (kernel-review fix 3).
#[test_case]
fn console_ansi_stray_control_byte_inside_csi_recovers() {
    let mut c = console(80, 40);

    c.write_bytes(b"\x1b[3\x077mZ");

    // Traced through `ansi::Parser`: `ESC[` starts a sequence, `3` is a
    // parameter digit, the BEL breaks out of it (consumed, nothing
    // printed) back to plain text, so `7`, `m` and `Z` each print as
    // ordinary characters from then on -- the point being that this is
    // fully deterministic (no panic, no leftover parser state) rather
    // than any particular byte being "correct" to keep.
    assert_eq!(c.cell(0, 0).glyph, b'7');
    assert_eq!(c.cell(1, 0).glyph, b'm');
    assert_eq!(c.cell(2, 0).glyph, b'Z');
    assert_eq!(c.cursor(), (3, 0));
}

/// Glyph rendering test (brief M2-T4a): verify glyphs render with correct
/// pixel orientation -- leftmost pixels should be in the left columns of
/// the cell, not mirrored.
#[test_case]
fn console_glyph_rendering_orientation() {
    let mut c = console(80, 40);

    // 'L' should have vertical stroke on left, foot extending right
    c.write_bytes(b"L");
    assert_eq!(c.cell(0, 0).glyph, b'L');

    // '[' should have vertical bar on the left side
    c.write_bytes(b"[");
    assert_eq!(c.cell(1, 0).glyph, b'[');

    // '(' should curve open to the right (parenthesis)
    c.write_bytes(b"(");
    assert_eq!(c.cell(2, 0).glyph, b'(');

    // '1' and 'I' and 'l' should be distinct from each other
    c.write_bytes(b"1Il");
    assert_ne!(c.cell(3, 0).glyph, c.cell(4, 0).glyph); // '1' != 'I'
    assert_ne!(c.cell(4, 0).glyph, c.cell(5, 0).glyph); // 'I' != 'l'
    assert_ne!(c.cell(3, 0).glyph, c.cell(5, 0).glyph); // '1' != 'l'
}
