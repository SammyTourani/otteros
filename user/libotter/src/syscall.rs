//! Raw syscall numbers, the `syscall` instruction wrappers, and errno
//! constants (DECISIONS.md D16, `SYSCALLS.md`). Every other module in this
//! crate goes through `raw::syscallN` here rather than embedding its own
//! `asm!` block.

pub mod num {
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const GETPID: u64 = 3;
    pub const YIELD: u64 = 4;
    pub const SLEEP_MS: u64 = 5;
    pub const MAP_ANON: u64 = 6;
    pub const UNMAP: u64 = 7;
    pub const TIME_MS: u64 = 8;
    pub const SPAWN: u64 = 9;
    pub const WAIT: u64 = 10;
    pub const KILL: u64 = 11;
    pub const DEBUG_LOG: u64 = 12;
    pub const PROC_LIST: u64 = 13;
    pub const SYSINFO: u64 = 14;
    pub const REBOOT: u64 = 15;
    pub const TEST_EXIT: u64 = 16;
    pub const GETRANDOM: u64 = 17;
}

/// Real Linux numeric values (`SYSCALLS.md`), so this can double as a
/// borrow-free error type: a syscall wrapper returns `Result<u64, Errno>`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Errno(pub i32);

impl Errno {
    pub const EPERM: Errno = Errno(1);
    pub const ENOENT: Errno = Errno(2);
    pub const ESRCH: Errno = Errno(3);
    pub const E2BIG: Errno = Errno(7);
    pub const ENOEXEC: Errno = Errno(8);
    pub const ECHILD: Errno = Errno(10);
    pub const EAGAIN: Errno = Errno(11);
    pub const ENOMEM: Errno = Errno(12);
    pub const EFAULT: Errno = Errno(14);
    pub const EINVAL: Errno = Errno(22);
    pub const ENOSYS: Errno = Errno(38);
}

/// Turns a raw `rax` result (D16: `-1..=-4095` is `-errno`) into
/// `Result<u64, Errno>`.
fn result(raw: i64) -> Result<u64, Errno> {
    if (-4095..0).contains(&raw) { Err(Errno(-raw as i32)) } else { Ok(raw as u64) }
}

/// # Safety
/// `num` must name a real syscall whose calling convention matches "no
/// arguments" (D16).
pub unsafe fn syscall0(num: u64) -> i64 {
    let ret: i64;
    // SAFETY: forwarded from this function's own contract; `rcx`/`r11` are
    // clobbered by the `syscall` instruction itself (D16) and declared as
    // such below so the compiler never assumes they survive.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

/// # Safety
/// `num` must name a real syscall taking exactly one argument (D16), and
/// `a1` must be whatever that argument's own contract requires (e.g. a
/// valid pointer, if it's used as one).
pub unsafe fn syscall1(num: u64, a1: u64) -> i64 {
    let ret: i64;
    // SAFETY: see `syscall0`.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") a1,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

/// # Safety
/// Same as `syscall1`, extended to two arguments.
pub unsafe fn syscall2(num: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    // SAFETY: see `syscall0`.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") a1,
            in("rsi") a2,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

/// # Safety
/// Same as `syscall1`, extended to three arguments.
pub unsafe fn syscall3(num: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    // SAFETY: see `syscall0`.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

/// # Safety
/// Same as `syscall1`, extended to four arguments. `a4` goes in `r10`, not
/// `rcx` (D16 -- `rcx` is clobbered by `syscall` itself).
pub unsafe fn syscall4(num: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let ret: i64;
    // SAFETY: see `syscall0`.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") num => ret,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    ret
}

/// # Safety
/// `num` must name a real syscall matching the argument count/contract
/// used here.
pub unsafe fn call0(num: u64) -> Result<u64, Errno> {
    // SAFETY: forwarded.
    result(unsafe { syscall0(num) })
}

/// # Safety
/// See `call0`.
pub unsafe fn call1(num: u64, a1: u64) -> Result<u64, Errno> {
    // SAFETY: forwarded.
    result(unsafe { syscall1(num, a1) })
}

/// # Safety
/// See `call0`.
pub unsafe fn call2(num: u64, a1: u64, a2: u64) -> Result<u64, Errno> {
    // SAFETY: forwarded.
    result(unsafe { syscall2(num, a1, a2) })
}

/// # Safety
/// See `call0`.
pub unsafe fn call3(num: u64, a1: u64, a2: u64, a3: u64) -> Result<u64, Errno> {
    // SAFETY: forwarded.
    result(unsafe { syscall3(num, a1, a2, a3) })
}

/// # Safety
/// See `call0`.
pub unsafe fn call4(num: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> Result<u64, Errno> {
    // SAFETY: forwarded.
    result(unsafe { syscall4(num, a1, a2, a3, a4) })
}
