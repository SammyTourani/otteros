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

/// Exits QEMU with a specific code via `isa-debug-exit` (brief M2-T4, syscall 16).
/// Used by userspace test_exit() syscall. Code 0 is success, non-zero is failure.
pub fn test_exit(code: i32) -> ! {
    // Map the user exit code to an isa-debug-exit code: 0 -> success, non-zero -> failure
    let exit_byte = if code == 0 { SUCCESS_CODE } else { FAILURE_CODE };
    // SAFETY: same as exit() above -- we're using the isa-debug-exit device.
    unsafe { outb(EXIT_PORT, exit_byte) };
    crate::hlt_loop();
}

/// Reboots the system (brief M2-T4, syscall 15).
/// Uses the 8042 reset method (0xFE to port 0x64).
pub fn reboot() -> ! {
    // Try 8042 reset: send 0xFE to port 0x64 (keyboard controller command port).
    // Brief M2-T4: one of these always succeeds: ACPI FADT reset, 8042 pulse (this), or triple fault.
    // SAFETY: port 0x64 is the i8042 command port, 0xFE is the reset command documented
    // in the i8042 datasheet. This is the standard method for soft reboot on x86.
    unsafe { outb(0x64, 0xfe) };

    // Wait for reboot
    for _ in 0..1_000_000 {
        core::hint::spin_loop();
    }

    // Should never reach here, but halt just in case
    crate::hlt_loop();
}
