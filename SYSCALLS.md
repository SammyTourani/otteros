# OtterOS syscall table (M2-T2)

ABI (DECISIONS.md D16): `syscall`/`sysret`. Number in `rax`; arguments in
`rdi, rsi, rdx, r10, r8, r9` (note `r10`, not `rcx` — `rcx`/`r11` are
clobbered by the `syscall`/`sysret` instructions themselves). Result in
`rax`; negative values `-1..=-4095` are `-errno` (Linux convention).

This table must stay in sync with `kernel/src/syscall/table.rs`.

| # | Name | Args | Returns | Notes |
|---|------|------|---------|-------|
| 0 | `exit` | `code: i32` | never returns | Ends the calling process; frees its address space. |
| 1 | `write` | `fd: u64, buf: *const u8, len: u64` | bytes written, or `-errno` | `fd` 1/2 only, both go to the console *and* serial. |
| 2 | `read` | `fd: u64, buf: *mut u8, len: u64` | bytes read, or `-errno` | `fd` 0 only: one blocking keyboard character. |
| 3 | `getpid` | — | pid | |
| 4 | `yield` | — | `0` | Voluntarily gives up the CPU. |
| 5 | `sleep_ms` | `ms: u64` | `0` | |
| 6 | `map_anon` | `len: u64` | address, or `-errno` | Anonymous, writable, non-executable memory. |
| 7 | `unmap` | `addr: u64, len: u64` | `0`, or `-errno` | `addr`/`len` must be 4 KiB aligned. |
| 8 | `time_ms` | — | milliseconds since boot | |
| 9 | `spawn` | — | `-ENOSYS` | Implemented in M2-T3 (ELF loader). |
| 10 | `wait` | `pid: u64` | exit code, or `-errno` | Blocks until `pid` exits; collects the zombie. |
| 11 | `kill` | `pid: u64` | `0`, or `-errno` | Ends another process; self-`kill` behaves like `exit`. |
| 12 | `debug_log` | `buf: *const u8, len: u64` | bytes written, or `-errno` | Serial only, never the console. |

Any other number returns `-ENOSYS`.

## Errno values (`kernel/src/syscall/errno.rs`)

Real Linux numeric values, so a userspace helper library can reuse them
directly.

| Name | Value |
|------|-------|
| `EPERM` | 1 |
| `ENOENT` | 2 |
| `ESRCH` | 3 |
| `ECHILD` | 10 |
| `EAGAIN` | 11 |
| `ENOMEM` | 12 |
| `EFAULT` | 14 |
| `EINVAL` | 22 |
| `ENOSYS` | 38 |
