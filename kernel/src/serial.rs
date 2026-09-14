//! 16550 UART driver on COM1 (I/O port 0x3F8) -- the kernel's primary log
//! channel (DECISIONS.md D5). Polling only; no interrupts (that's later).

use core::fmt;

use crate::arch::x86_64::port::{inb, outb};
use crate::sync::IrqMutex;

const COM1: u16 = 0x3F8;

/// Line Status Register bit 5: the transmit holding register is empty and
/// ready for the next byte.
const LSR_THR_EMPTY: u8 = 0x20;

/// Reads the Line Status Register for the UART at `base`. Always
/// side-effect free for our purposes; shared by the locked `SerialPort`
/// path and the lock-free `EmergencyWriter` path below.
fn raw_line_status(base: u16) -> u8 {
    // SAFETY: `base + 5` is the LSR of a 16550-compatible UART; reading it
    // has no side effects.
    unsafe { inb(base + 5) }
}

/// Blocks until the transmit holding register for the UART at `base` is
/// empty, then sends one byte. No locking: the caller is responsible for
/// either holding `SERIAL1` (`SerialPort::write_byte`) or deliberately not
/// (`EmergencyWriter`, see its docs).
fn raw_write_byte(base: u16, byte: u8) {
    while raw_line_status(base) & LSR_THR_EMPTY == 0 {
        core::hint::spin_loop();
    }
    // SAFETY: we just confirmed (LSR bit 5) that the transmit holding
    // register is empty, so writing the data register at `base` is exactly
    // what the 16550 protocol expects here.
    unsafe { outb(base, byte) };
}

/// Whether `SerialPort::write_str` is currently inside an ANSI/VT escape
/// sequence it should swallow rather than forward to the wire (brief
/// M1-T7 fix 2: `console` needs those for colour, but `artifacts/serial.log`
/// -- what the test harness and a human tailing serial both actually read
/// -- never should). A deliberately minimal mirror of `console::ansi`'s
/// state machine: this only needs to know *where a sequence ends*, never
/// what it means, so there's no parameter/attribute tracking at all.
/// Lives on `SerialPort` itself (not a local in `write_str`) so it
/// persists across separate `write_str` calls -- and therefore across
/// separate `kprint!`/`kprintln!` calls -- the same way `console::ansi::Parser`
/// persists across `Console::write_bytes` calls, in case a sequence is
/// ever split across two of them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnsiFilterState {
    Normal,
    /// Just saw `ESC` (0x1b); the next byte decides CSI (`[`) or not.
    Escape,
    /// `ESC[` seen; swallowing bytes until a CSI final byte (0x40..=0x7e).
    Csi,
}

pub struct SerialPort {
    base: u16,
    ansi_filter: AnsiFilterState,
}

impl SerialPort {
    pub const fn new(base: u16) -> Self {
        Self { base, ansi_filter: AnsiFilterState::Normal }
    }

    /// Programs the UART for 38400 8N1 with the FIFOs enabled. Idempotent.
    pub fn init(&mut self) {
        // SAFETY: `self.base` is COM1's fixed, well-known I/O port range,
        // always present as a 16550-compatible UART under QEMU. This is the
        // standard init sequence: mask the UART's own interrupts (we poll),
        // set the baud-rate divisor via DLAB, select 8N1 framing, then
        // enable+clear the FIFOs and assert the modem-control lines some
        // hosts expect to see driven.
        unsafe {
            outb(self.base + 1, 0x00); // IER: disable all UART interrupts
            outb(self.base + 3, 0x80); // LCR: enable DLAB to program the divisor
            outb(self.base, 0x03); // divisor low byte: 115200 / 3 = 38400 baud
            outb(self.base + 1, 0x00); // divisor high byte
            outb(self.base + 3, 0x03); // LCR: 8 bits, no parity, 1 stop bit (clears DLAB)
            outb(self.base + 2, 0xC7); // FCR: enable FIFOs, clear them, 14-byte threshold
            outb(self.base + 4, 0x0B); // MCR: RTS/DSR set, OUT2 enabled
        }
    }

    fn line_status(&self) -> u8 {
        raw_line_status(self.base)
    }

    /// Blocks until the transmit holding register is empty, then sends one
    /// byte.
    pub fn write_byte(&mut self, byte: u8) {
        raw_write_byte(self.base, byte);
    }

    /// Polls (briefly) for the transmit holding register to report empty
    /// again after a write, i.e. that the byte was actually accepted for
    /// sending. Used by the serial self-test.
    fn wait_transmitter_idle(&self, attempts: u32) -> bool {
        for _ in 0..attempts {
            if self.line_status() & LSR_THR_EMPTY != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Feeds one byte through the ANSI-sequence filter; returns `true` if
    /// it's an ordinary byte `write_str` should forward to the wire,
    /// `false` if it was consumed as part of an escape sequence instead.
    fn filter_ansi(&mut self, byte: u8) -> bool {
        match self.ansi_filter {
            AnsiFilterState::Normal => {
                if byte == 0x1b {
                    self.ansi_filter = AnsiFilterState::Escape;
                    false
                } else {
                    true
                }
            }
            AnsiFilterState::Escape => {
                // Nothing outside CSI (`ESC[...]`) sequences reaches
                // serial either -- `console::ansi::Parser` swallows the
                // one byte after a non-`[` `ESC` the same way.
                self.ansi_filter = if byte == b'[' { AnsiFilterState::Csi } else { AnsiFilterState::Normal };
                false
            }
            AnsiFilterState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.ansi_filter = AnsiFilterState::Normal;
                }
                false
            }
        }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if !self.filter_ansi(byte) {
                continue;
            }
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

/// The COM1 port. `IrqMutex::new` and `SerialPort::new` are both `const
/// fn`, so this needs no lazy-initialisation crate (see DECISIONS.md D2):
/// the UART itself is programmed later, from `init()`. `IrqMutex` (brief
/// M1-T5), not a bare `spin::Mutex`: once interrupts are enabled for good,
/// a timer tick landing while this core already holds `SERIAL1` (e.g. a
/// panic mid-`kprintln!`, or simply an unlucky tick) must not be able to
/// call `kprintln!` itself and deadlock against the very lock it
/// interrupted.
pub static SERIAL1: IrqMutex<SerialPort> = IrqMutex::new(SerialPort::new(COM1));

/// Brings up the COM1 UART. Must run before anything else logs.
pub fn init() {
    SERIAL1.lock().init();
}

/// Writes a byte and confirms the UART reports it as accepted for sending.
/// A real (if narrow) hardware check, exercised by a `#[test_case]` in
/// `test_main.rs`.
pub fn self_test() -> bool {
    let mut serial = SERIAL1.lock();
    serial.write_byte(b'.');
    serial.wait_transmitter_idle(10_000)
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use fmt::Write;
    SERIAL1
        .lock()
        .write_fmt(args)
        .expect("serial write_fmt should never fail");
    // Brief M1-T7 step 3: serial stays the first sink (tests read it, and
    // it can never itself be the reason a message is lost), but every
    // `kprint!`/`kprintln!` now reaches the screen too -- directly, once
    // `console::init` has run, or via `console`'s own boot-time ring
    // before that. Never called from IRQ context (see `console::feed`'s
    // own docs), so this can't race a handler for the console lock either.
    crate::console::feed(args);
}

/// Prints to the serial console, like `print!`.
#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => ($crate::serial::_print(core::format_args!($($arg)*)));
}

/// Prints to the serial console with a trailing newline, like `println!`.
#[macro_export]
macro_rules! kprintln {
    () => ($crate::kprint!("\n"));
    ($($arg:tt)*) => ($crate::kprint!("{}\n", core::format_args!($($arg)*)));
}

/// A COM1 writer that never takes `SERIAL1`'s lock.
///
/// `kprint!`/`kprintln!` go through `SERIAL1: IrqMutex<SerialPort>`,
/// which is correct for normal logging but deadlocks forever if the code
/// that faults is the very code holding that lock: `IrqMutex` disables
/// maskable interrupts for the duration of the critical section (brief
/// M1-T5), but an NMI, or a synchronous CPU exception (a bug taking a
/// page fault mid-`kprintln!`, say), can still be taken *while*
/// `SERIAL1.lock()` is held -- there's no other core yet, but a
/// re-entrant fault is exactly a single core taking a second lock it
/// already holds, and the `spin::Mutex` `IrqMutex` wraps is not
/// reentrant. The fault/panic paths
/// (`trap::trap_dispatch`, the panic handler) must never be able to
/// deadlock on their way to reporting a fault, so they use this instead:
/// raw, unlocked port writes straight to the hardware.
///
/// This is unsound to use *concurrently* with itself or with `SERIAL1` in
/// the sense that bytes from two writers racing on the same UART can
/// interleave -- but that's an accepted, deliberate tradeoff here: a
/// garbled-but-present fault message beats a clean deadlock, and in
/// practice the only writer left running by the time this is used is
/// whichever fault handler called it (single core, interrupts handled by
/// our own dispatcher, not reentered except for the nested-fault case
/// `trap.rs` detects and halts on separately).
pub struct EmergencyWriter;

impl fmt::Write for EmergencyWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                raw_write_byte(COM1, b'\r');
            }
            raw_write_byte(COM1, byte);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print_emergency(args: fmt::Arguments) {
    use fmt::Write;
    // A formatting error here has no sane fallback (we're already on the
    // fault path); best effort and move on rather than panicking again.
    let _ = EmergencyWriter.write_fmt(args);
}

/// Like `kprint!`, but never takes `SERIAL1`'s lock (see `EmergencyWriter`).
/// Only for the fault/panic paths.
#[macro_export]
macro_rules! kprint_emergency {
    ($($arg:tt)*) => ($crate::serial::_print_emergency(core::format_args!($($arg)*)));
}

/// Like `kprintln!`, but never takes `SERIAL1`'s lock (see
/// `EmergencyWriter`). Only for the fault/panic paths.
#[macro_export]
macro_rules! kprintln_emergency {
    () => ($crate::kprint_emergency!("\n"));
    ($($arg:tt)*) => ($crate::kprint_emergency!("{}\n", core::format_args!($($arg)*)));
}
