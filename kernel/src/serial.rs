//! 16550 UART driver on COM1 (I/O port 0x3F8) -- the kernel's primary log
//! channel (DECISIONS.md D5). Polling only; no interrupts (that's later).

use core::fmt;

use spin::Mutex;

use crate::arch::x86_64::port::{inb, outb};

const COM1: u16 = 0x3F8;

/// Line Status Register bit 5: the transmit holding register is empty and
/// ready for the next byte.
const LSR_THR_EMPTY: u8 = 0x20;

pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    pub const fn new(base: u16) -> Self {
        Self { base }
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
        // SAFETY: reading the Line Status Register is always side-effect
        // free for our purposes; `self.base + 5` is the LSR for this UART.
        unsafe { inb(self.base + 5) }
    }

    /// Blocks until the transmit holding register is empty, then sends one
    /// byte.
    pub fn write_byte(&mut self, byte: u8) {
        while self.line_status() & LSR_THR_EMPTY == 0 {
            core::hint::spin_loop();
        }
        // SAFETY: we just confirmed (LSR bit 5) that the transmit holding
        // register is empty, so writing the data register at `self.base` is
        // exactly what the 16550 protocol expects here.
        unsafe { outb(self.base, byte) };
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
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

/// The COM1 port. `Mutex::new` and `SerialPort::new` are both `const fn`, so
/// this needs no lazy-initialisation crate (see DECISIONS.md D2): the UART
/// itself is programmed later, from `init()`.
pub static SERIAL1: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1));

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
