//! Minimal x86 port I/O primitives (`in`/`out` on an 8-bit port). Everything
//! we talk to in M0 -- the 16550 UART and the isa-debug-exit device -- is
//! byte-wide, so that's all this module provides.

/// Reads a byte from an I/O port.
///
/// # Safety
/// The caller must ensure `port` names a hardware I/O port that is safe to
/// read at this point in execution: the expected device is actually present
/// there, and reading it has no effect the caller hasn't accounted for.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: a single `in` from `port`, forwarded from the caller's own
    // safety contract; no memory is touched and flags are unaffected.
    unsafe {
        core::arch::asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

/// Writes a byte to an I/O port.
///
/// # Safety
/// The caller must ensure `port` names a hardware I/O port that is safe to
/// write at this point in execution, and that `value` is meaningful there
/// per that device's documented protocol.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY: a single `out` to `port`, forwarded from the caller's own
    // safety contract; no memory is touched and flags are unaffected.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}
