//! `print!`/`println!`/`eprintln!` on fd 1/2, and a blocking `read_line` on
//! fd 0 (`SYSCALLS.md` 0/1/2, brief M2-T3 step 5).

use core::fmt;

use crate::syscall::{self, num};

/// The kernel's own per-call cap on `write`/`read`/`debug_log`
/// (`syscall::table::MAX_IO_LEN`) -- a single call larger than this is
/// flatly rejected (`-EINVAL`), not truncated, so `write_fd` must chunk
/// *before* calling the syscall, never rely on a partial-write return to
/// do it for us.
const MAX_IO_LEN: usize = 4096;

pub struct Stdout;
pub struct Stderr;

impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_fd(1, s.as_bytes());
        Ok(())
    }
}

impl fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_fd(2, s.as_bytes());
        Ok(())
    }
}

fn write_fd(fd: u64, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let take = bytes.len().min(MAX_IO_LEN);
        // SAFETY: `WRITE` takes `(fd, buf, len)`; `bytes[..take]` is a
        // valid slice for the duration of this call.
        let result = unsafe { syscall::call3(num::WRITE, fd, bytes.as_ptr() as u64, take as u64) };
        match result {
            Ok(0) | Err(_) => return, // fd 1/2 never really fail, but stop rather than spin if one somehow does.
            Ok(n) => bytes = &bytes[(n as usize).min(bytes.len())..],
        }
    }
}

/// Reads one blocking line from fd 0 (`SYSCALLS.md` 2: one character per
/// syscall), stopping at `'\n'` (consumed, not stored) or once `buf` is
/// full, whichever comes first. Returns how many bytes were written into
/// `buf`.
pub fn read_line(buf: &mut [u8]) -> usize {
    let mut n = 0;
    while n < buf.len() {
        let mut byte = [0u8; 1];
        // SAFETY: `READ` takes `(fd, buf, len)`; `byte` is a valid 1-byte
        // buffer for the duration of this call.
        let Ok(1) = (unsafe { syscall::call3(num::READ, 0, byte.as_mut_ptr() as u64, 1) }) else { break };
        if byte[0] == b'\n' {
            break;
        }
        buf[n] = byte[0];
        n += 1;
    }
    n
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        let _ = write!($crate::io::Stdout, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        let _ = writeln!($crate::io::Stdout, $($arg)*);
    }};
}

#[macro_export]
macro_rules! eprintln {
    () => {{
        use core::fmt::Write as _;
        let _ = writeln!($crate::io::Stderr);
    }};
    ($($arg:tt)*) => {{
        use core::fmt::Write as _;
        let _ = writeln!($crate::io::Stderr, $($arg)*);
    }};
}
