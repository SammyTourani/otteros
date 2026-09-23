//! The syscall table (brief M2-T2): number in `rax`, arguments in `rdi,
//! rsi, rdx, r10, r8, r9` (D16 -- `r10`, not `rcx`, since `rcx` is
//! clobbered by the `syscall` instruction itself). Numbers and behaviour
//! are documented in `SYSCALLS.md` at the repo root; keep the two in sync.

use alloc::string::String;

use super::errno;
use crate::arch::x86_64::syscall_entry::SyscallFrame;
use crate::drivers::ps2::keyboard;
use crate::proc::process::MapAnonError;
use crate::proc::{self, usermem};
use crate::sched;

/// A single `read`/`write`/`debug_log` call never copies more than this
/// many bytes in or out at once -- a defensive bound (not specified by the
/// brief) so a hostile/buggy `len` can't force an unbounded kernel-side
/// buffer; everything M2-T2's own payloads ever transfer is a handful of
/// bytes.
const MAX_IO_LEN: usize = 4096;

/// Reads `frame`'s syscall number and D16 argument registers and
/// dispatches to the matching handler, returning the raw `rax` result
/// (D16: negative `-1..=-4095` is `-errno`). `0` (`exit`) never returns
/// here at all -- see `sys_exit`.
pub(crate) fn dispatch(frame: &mut SyscallFrame) -> i64 {
    let (a0, a1, a2) = (frame.rdi, frame.rsi, frame.rdx);
    match frame.rax {
        0 => sys_exit(a0 as i32),
        1 => sys_write(a0, a1, a2),
        2 => sys_read(a0, a1, a2),
        3 => sys_getpid(),
        4 => sys_yield(),
        5 => sys_sleep_ms(a0),
        6 => sys_map_anon(a0),
        7 => sys_unmap(a0, a1),
        8 => sys_time_ms(),
        9 => err(errno::ENOSYS), // spawn: implemented in M2-T3.
        10 => sys_wait(a0),
        11 => sys_kill(a0),
        12 => sys_debug_log(a0, a1),
        _ => err(errno::ENOSYS),
    }
}

fn err(e: u32) -> i64 {
    -(i64::from(e))
}

/// Syscall 0: never returns -- ends the calling process outright.
fn sys_exit(code: i32) -> i64 {
    proc::exit_current_process(code)
}

/// Syscall 1: `write(fd, buf, len)`. `fd` 1/2 both go to the console *and*
/// serial (brief M2-T2 step 7) -- the same `kprint!` path the kernel's own
/// logging uses, since that already does exactly "console + serial".
fn sys_write(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != 1 && fd != 2 {
        return err(errno::EINVAL);
    }
    let Some(len) = clamp_io_len(len) else { return err(errno::EINVAL) };
    if len == 0 {
        return 0;
    }

    let process = proc::current();
    let mut buffer = [0u8; MAX_IO_LEN];
    if usermem::copy_from_user(&process.address_space(), &mut buffer[..len], buf).is_err() {
        return err(errno::EFAULT);
    }
    crate::kprint!("{}", String::from_utf8_lossy(&buffer[..len]));
    len as i64
}

/// Syscall 2: `read(fd, buf, len)`. `fd` 0 reads one blocking keyboard
/// character (brief M2-T2 step 7); nothing else is supported yet.
///
/// Kernel-review round 2: the destination is validated (present, `USER`,
/// `WRITABLE`) *before* the blocking keyboard read, not after -- a bad
/// buffer must come back as `-EFAULT` immediately, not hang forever
/// waiting for a keypress nobody can act on until this call already
/// returns.
fn sys_read(fd: u64, buf: u64, len: u64) -> i64 {
    if fd != 0 {
        return err(errno::EINVAL);
    }
    if len == 0 {
        return 0;
    }
    let process = proc::current();
    if usermem::validate_writable(&process.address_space(), buf, 1).is_err() {
        return err(errno::EFAULT);
    }
    let ch = keyboard::read_char_blocking();
    let byte = [ch as u8];
    if usermem::copy_to_user(&process.address_space(), buf, &byte).is_err() {
        return err(errno::EFAULT);
    }
    1
}

/// Syscall 3: `getpid()`.
fn sys_getpid() -> i64 {
    proc::current().pid() as i64
}

/// Syscall 4: `yield()`.
fn sys_yield() -> i64 {
    sched::yield_now();
    0
}

/// Syscall 5: `sleep_ms(ms)`.
fn sys_sleep_ms(ms: u64) -> i64 {
    sched::sleep_ms(ms);
    0
}

/// Syscall 6: `map_anon(len) -> addr`. Bounds (non-zero, page-rounded with
/// checked arithmetic, 1 GiB cap, never reaching the stack region or
/// `usermem::USER_SPACE_CEILING`) are enforced by `Process::map_anon`
/// itself (kernel-review round 2).
fn sys_map_anon(len: u64) -> i64 {
    let Ok(len) = usize::try_from(len) else { return err(errno::EINVAL) };
    match proc::current().map_anon(len) {
        Ok(addr) => addr.as_u64() as i64,
        Err(MapAnonError::Invalid) => err(errno::EINVAL),
        Err(MapAnonError::OutOfMemory) => err(errno::ENOMEM),
    }
}

/// Syscall 7: `unmap(addr, len)`. Bounds (alignment, non-zero, fully
/// inside the process's own user half, every page actually mapped) are
/// enforced by `Process::unmap` itself (kernel-review round 2).
fn sys_unmap(addr: u64, len: u64) -> i64 {
    match proc::current().unmap(addr, len) {
        Ok(()) => 0,
        Err(()) => err(errno::EINVAL),
    }
}

/// Syscall 8: `time_ms()`.
fn sys_time_ms() -> i64 {
    crate::time::uptime_ms() as i64
}

/// Syscall 10: `wait(pid) -> exit code`.
fn sys_wait(pid: u64) -> i64 {
    match proc::wait(pid) {
        Some(code) => i64::from(code),
        None => err(errno::ESRCH),
    }
}

/// Syscall 11: `kill(pid)`.
fn sys_kill(pid: u64) -> i64 {
    /// The same "128 + signal-like constant" shape `proc::fault` uses for
    /// a fault-killed process (`SIGKILL` = 9) -- `kill` has no real signal
    /// number to report either, this being neither Unix signals nor a
    /// real `SIGKILL` delivery, just "the process is gone now".
    const KILLED_EXIT_CODE: i32 = 128 + 9;

    let process = proc::current();
    if process.pid() == pid {
        // Never returns.
        proc::exit_current_process(KILLED_EXIT_CODE);
    }
    if proc::kill(pid, KILLED_EXIT_CODE) { 0 } else { err(errno::ESRCH) }
}

/// Syscall 12: `debug_log(buf, len)` -- serial only, never the console
/// (brief M2-T2 step 7).
fn sys_debug_log(buf: u64, len: u64) -> i64 {
    let Some(len) = clamp_io_len(len) else { return err(errno::EINVAL) };
    if len == 0 {
        return 0;
    }
    let process = proc::current();
    let mut buffer = [0u8; MAX_IO_LEN];
    if usermem::copy_from_user(&process.address_space(), &mut buffer[..len], buf).is_err() {
        return err(errno::EFAULT);
    }
    crate::serial::debug_log(&String::from_utf8_lossy(&buffer[..len]));
    len as i64
}

/// `len` as a `usize` no bigger than `MAX_IO_LEN`; `None` if it's larger
/// (a defensive cap, see `MAX_IO_LEN`'s own docs -- not an `-EFAULT`,
/// since the length itself, not the pointer, is what's out of range).
fn clamp_io_len(len: u64) -> Option<usize> {
    if len > MAX_IO_LEN as u64 { None } else { Some(len as usize) }
}
