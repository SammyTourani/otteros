# OtterOS syscall table (M2-T2, extended M2-T3)

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
| 9 | `spawn` | `path_ptr: *const u8, path_len: u64, argv_ptr: *const (u64, u64), argc: u64` | pid, or `-errno` | `argv_ptr` points at `argc` packed `(ptr, len)` pairs, one per argument string (not NUL-terminated C strings). Limits: path ≤ 256 bytes, ≤ 32 args, ≤ 4 KiB of argv bytes total (`-E2BIG` past any of these). Looks `path` up in the initramfs (`-ENOENT`), loads it as a static ELF64 executable (`-ENOEXEC` if invalid), builds a fresh address space + SysV-style initial stack for `argv` (DECISIONS.md D18). |
| 10 | `wait` | `pid: u64` | exit code, or `-errno` | Blocks until `pid` exits; collects the zombie. |
| 11 | `kill` | `pid: u64` | `0`, or `-errno` | Ends another process; self-`kill` behaves like `exit`. |
| 12 | `debug_log` | `buf: *const u8, len: u64` | bytes written, or `-errno` | Serial only, never the console. |
| 13 | `proc_list` | `buf: *mut u8, cap: u64` | bytes written, or `-errno` | Fills `buf` with process list entries `{pid: u64, ppid: u64, state: u32, name[16], ticks: u64}` (56 bytes each). Returns the number of bytes written. `-EINVAL` if `cap` < 56 or not aligned. `buf` must be writable user memory. |
| 14 | `sysinfo` | `buf: *mut u8` | bytes written, or `-errno` | Fills `buf` with sysinfo struct (64 bytes): `{uptime_ms: u64, total_frames: u64, free_frames: u64, heap_bytes: u64}`. Writes 64 bytes. |
| 15 | `reboot` | — | never returns | Reboots the system via ACPI FADT reset register, 8042 pulse (0xFE to port 0x64), or triple fault; always one of these succeeds. |
| 16 | `test_exit` | `code: i32` | never returns, or `-EPERM` | Honoured only if kernel was booted in test mode (cmdline contains `test`). Exits QEMU with the code via `isa-debug-exit` port 0xf4. Otherwise returns `-EPERM`. |
| 17 | `getrandom` | `buf: *mut u8, len: u64, flags: u32` | bytes written, or `-errno` | Fills `buf` with `len` random bytes from the kernel CSPRNG (brief M8-T6b). `flags` must be 0 (else `-EINVAL`). `len` ≤ 4096 (else `-EINVAL`). `buf` must be writable user memory (else `-EFAULT`). |

Any other number returns `-ENOSYS`.

## Errno values (`kernel/src/syscall/errno.rs`)

Real Linux numeric values, so a userspace helper library can reuse them
directly.

| Name | Value |
|------|-------|
| `EPERM` | 1 |
| `ENOENT` | 2 |
| `ESRCH` | 3 |
| `E2BIG` | 7 |
| `ENOEXEC` | 8 |
| `ECHILD` | 10 |
| `EAGAIN` | 11 |
| `ENOMEM` | 12 |
| `EFAULT` | 14 |
| `EINVAL` | 22 |
| `ENOSYS` | 38 |
