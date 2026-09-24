//! Syscall error numbers (brief M2-T2, DECISIONS.md D16): the Linux
//! convention's actual numeric values, so a userspace helper library can
//! reuse the same constants a Linux program would. `syscall::table::dispatch`
//! returns `-(errno as i64)` on failure (D16: "negative values -1..-4095
//! are `-errno`"); a successful call's result is always `>= 0`.

pub const EPERM: u32 = 1;
pub const ENOENT: u32 = 2;
pub const ESRCH: u32 = 3;
pub const E2BIG: u32 = 7;
pub const ENOEXEC: u32 = 8;
pub const ECHILD: u32 = 10;
pub const EAGAIN: u32 = 11;
pub const ENOMEM: u32 = 12;
pub const EFAULT: u32 = 14;
pub const EINVAL: u32 = 22;
pub const ENOSYS: u32 = 38;
