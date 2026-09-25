//! Process-control syscall wrappers (`SYSCALLS.md`): `exit`, `getpid`,
//! `yield_now`, `sleep_ms`, `time_ms`, `spawn`, `wait`, `kill`.

use crate::syscall::{self, Errno, num};

pub type Pid = u64;

/// Syscall 0: never returns.
pub fn exit(code: i32) -> ! {
    // SAFETY: `EXIT` takes one argument (the exit code) and, per
    // `SYSCALLS.md`, never returns to the caller.
    unsafe { syscall::syscall1(num::EXIT, code as i64 as u64) };
    unreachable!("libotter::process::exit: the kernel never returns from exit");
}

/// Syscall 3: `getpid()`.
pub fn getpid() -> Pid {
    // SAFETY: `GETPID` takes no arguments and always succeeds.
    unsafe { syscall::call0(num::GETPID) }.unwrap_or(0)
}

/// Syscall 4: `yield()`. Named `yield_now` (not `yield`, a Rust keyword).
pub fn yield_now() {
    // SAFETY: `YIELD` takes no arguments and always succeeds.
    let _ = unsafe { syscall::call0(num::YIELD) };
}

/// Syscall 5: `sleep_ms(ms)`.
pub fn sleep_ms(ms: u64) {
    // SAFETY: `SLEEP_MS` takes one argument and always succeeds.
    let _ = unsafe { syscall::call1(num::SLEEP_MS, ms) };
}

/// Syscall 8: `time_ms()` -- milliseconds since boot.
pub fn time_ms() -> u64 {
    // SAFETY: `TIME_MS` takes no arguments and always succeeds.
    unsafe { syscall::call0(num::TIME_MS) }.unwrap_or(0)
}

/// Syscall 9: `spawn(path, argv) -> pid`. `argv` does *not* need to
/// include `path` itself as `argv[0]` -- unlike POSIX `execve`, this
/// kernel's ABI keeps the program path and its argument list separate
/// (`SYSCALLS.md`); the spawned program's own `args()` sees exactly
/// `argv`, nothing prepended.
///
/// At most 32 arguments and 4 KiB of argument bytes combined
/// (`syscall::table::sys_spawn`'s own limits) -- a longer `args` returns
/// `Errno::E2BIG` without ever reaching the kernel's copy step.
pub fn spawn(path: &str, args: &[&str]) -> Result<Pid, Errno> {
    const MAX_ARGS: usize = 32;
    if args.len() > MAX_ARGS {
        return Err(Errno::E2BIG);
    }

    // One (ptr, len) pair per argument, laid out exactly as `sys_spawn`
    // expects to find them -- on this function's own stack, alive for the
    // duration of the syscall below (the kernel copies every string out of
    // user memory before returning, never retains a pointer into it).
    let mut table = [(0u64, 0u64); MAX_ARGS];
    for (slot, arg) in table.iter_mut().zip(args) {
        *slot = (arg.as_ptr() as u64, arg.len() as u64);
    }

    // SAFETY: `SPAWN` takes `(path_ptr, path_len, argv_ptr, argc)`; `path`
    // and `table[..args.len()]` are both valid for the duration of this
    // call.
    let pid = unsafe {
        syscall::call4(num::SPAWN, path.as_ptr() as u64, path.len() as u64, table.as_ptr() as u64, args.len() as u64)
    }?;
    Ok(pid)
}

/// Syscall 10: `wait(pid) -> exit code`. Blocks until `pid` exits.
pub fn wait(pid: Pid) -> Result<i32, Errno> {
    // SAFETY: `WAIT` takes one argument and returns the exit code or a
    // negative errno. Every process this crate's own programs ever spawn
    // exits with a small, non-negative code, so there's no ambiguity here
    // with D16's own `-1..=-4095` "-errno" convention -- a process that
    // deliberately exited with, say, -14 would be indistinguishable from
    // `wait` itself failing with `EFAULT`, but nothing in this codebase
    // ever does that through the real syscall ABI.
    let code = unsafe { syscall::call1(num::WAIT, pid) }?;
    Ok(code as i32)
}

/// Syscall 11: `kill(pid)`.
pub fn kill(pid: Pid) -> Result<(), Errno> {
    // SAFETY: `KILL` takes one argument and returns `0` or a negative errno.
    unsafe { syscall::call1(num::KILL, pid) }?;
    Ok(())
}

/// Syscall 13: `proc_list(buf, cap) -> bytes_written`. Fills `buf` with
/// process list entries (56 bytes each):
/// `{pid: u64, ppid: u64, state: u32, name[16], ticks: u64}`.
pub fn proc_list(buf: &mut [u8]) -> Result<usize, Errno> {
    // SAFETY: `PROC_LIST` takes `(buf, cap)`; `buf` is valid for the duration
    // of this call.
    let bytes = unsafe { syscall::call2(num::PROC_LIST, buf.as_mut_ptr() as u64, buf.len() as u64) }?;
    Ok(bytes as usize)
}

/// Syscall 14: `sysinfo(buf)`. Fills `buf` with sysinfo struct (64 bytes):
/// `{uptime_ms: u64, total_frames: u64, free_frames: u64, heap_bytes: u64}`.
pub fn sysinfo(buf: &mut [u8; 64]) -> Result<(), Errno> {
    // SAFETY: `SYSINFO` takes one argument (buf); `buf` is valid for the
    // duration of this call.
    unsafe { syscall::call1(num::SYSINFO, buf.as_mut_ptr() as u64) }?;
    Ok(())
}

/// Syscall 15: `reboot()`. Reboots the system. Never returns.
pub fn reboot() -> ! {
    // SAFETY: `REBOOT` takes no arguments and never returns.
    unsafe { syscall::syscall0(num::REBOOT) };
    unreachable!("libotter::process::reboot: the kernel never returns")
}

/// Syscall 16: `test_exit(code)`. Exits QEMU via isa-debug-exit in test mode;
/// returns `-EPERM` in normal mode.
pub fn test_exit(code: i32) -> Result<(), Errno> {
    // SAFETY: `TEST_EXIT` takes one argument (exit code) and either never
    // returns (in test mode) or returns an error code.
    match unsafe { syscall::syscall1(num::TEST_EXIT, code as i64 as u64) } {
        r if (-4095..0).contains(&r) => Err(Errno(-r as i32)),
        _ => unreachable!("libotter::process::test_exit: kernel never returns in test mode"),
    }
}

/// Syscall 17: `getrandom(buf, len, flags) -> bytes_written`. Fills `buf` with
/// random bytes from the kernel CSPRNG. Flags must be 0 (no flags supported).
/// At most 4096 bytes can be requested per call.
pub fn getrandom(buf: &mut [u8]) -> Result<usize, Errno> {
    let len = buf.len();
    // SAFETY: `GETRANDOM` takes `(buf_ptr, len, flags)`; `buf` is valid for the
    // duration of this call. Flags is 0 for standard behavior.
    let bytes = unsafe { syscall::call3(num::GETRANDOM, buf.as_mut_ptr() as u64, len as u64, 0) }?;
    Ok(bytes as usize)
}
