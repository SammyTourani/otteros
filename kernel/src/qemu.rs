//! Exiting QEMU via the `isa-debug-exit` device (DECISIONS.md D5).

use crate::arch::x86_64::port::outb;

const EXIT_PORT: u16 = 0xf4;
const SUCCESS_CODE: u8 = 0x10;
const FAILURE_CODE: u8 = 0x11;

/// Terminates the QEMU process through `isa-debug-exit`. QEMU turns the
/// written byte into its own process exit code as `(code << 1) | 1`, i.e.
/// 33 for `success` and 35 otherwise (see scripts/qemu.py).
///
/// Never returns: on the off chance the device isn't there (no such thing
/// on real hardware), we park the CPU instead of falling through with
/// whatever state we happened to be in.
pub fn exit(success: bool) -> ! {
    let code = if success { SUCCESS_CODE } else { FAILURE_CODE };
    // SAFETY: port 0xf4 is the isa-debug-exit device our QEMU harness always
    // attaches (`-device isa-debug-exit,iobase=0xf4,iosize=0x04`, see
    // scripts/qemu.py). Writing the documented exit code is exactly what
    // that device is for, and it terminates the QEMU process immediately.
    unsafe { outb(EXIT_PORT, code) };
    crate::hlt_loop();
}
