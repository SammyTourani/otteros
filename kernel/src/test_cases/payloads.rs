//! Tiny, hand-assembled ring-3 test payloads (brief M2-T2 step 11): each
//! is the *entire* code page for one process (mapped executable+user at
//! `arch::x86_64::usermode::ENTRY_RIP`, D18) -- no ELF, no libc, no
//! startup code, just raw machine code exercising one specific mechanism.
//!
//! Every one of these was produced by actually assembling and
//! disassembling the mnemonics below (`rustc --target x86_64-unknown-none
//! --emit=obj` over a `global_asm!` block, then `llvm-objdump -d` to
//! confirm the encoding byte-for-byte) rather than hand-encoding opcodes
//! from memory -- the comment above each array is that verified
//! disassembly, not just an aspiration. Syscall numbers match
//! `syscall::table` / `SYSCALLS.md`: 0 exit, 1 write, 999 (deliberately
//! unassigned, for `NOSYS`).

/// `hello`: writes "hi from ring 3\n" to fd 1, then exits with code 7.
///
/// ```text
/// mov rax, 1                       ; write
/// mov rdi, 1                       ; fd 1
/// lea rsi, [rip + hello_msg]
/// mov rdx, 15
/// syscall
/// mov rax, 0                       ; exit
/// mov rdi, 7
/// syscall
/// 1: jmp 1b                        ; unreachable safety net
/// hello_msg: .ascii "hi from ring 3\n"
/// ```
pub const HELLO: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
    0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00, // mov rdi, 1
    0x48, 0x8d, 0x35, 0x1b, 0x00, 0x00, 0x00, // lea rsi, [rip + 0x1b]  -> hello_msg
    0x48, 0xc7, 0xc2, 0x0f, 0x00, 0x00, 0x00, // mov rdx, 15
    0x0f, 0x05, // syscall
    0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
    0x48, 0xc7, 0xc7, 0x07, 0x00, 0x00, 0x00, // mov rdi, 7
    0x0f, 0x05, // syscall
    0xeb, 0xfe, // 1: jmp 1b
    0x68, 0x69, 0x20, 0x66, 0x72, 0x6f, 0x6d, 0x20, 0x72, 0x69, 0x6e, 0x67, 0x20, 0x33,
    0x0a, // "hi from ring 3\n"
];

/// `fault`: reads from address 0, which is never mapped -- expects the
/// kernel to log a `PAGE FAULT` kill and never come back here.
///
/// ```text
/// xor eax, eax
/// mov al, [rax]                    ; read from address 0
/// 1: jmp 1b                        ; unreachable safety net
/// ```
pub const FAULT: &[u8] = &[
    0x31, 0xc0, // xor eax, eax
    0x8a, 0x00, // mov al, [rax]
    0xeb, 0xfe, // 1: jmp 1b
];

/// `spin`: an infinite loop that never yields or calls a syscall --
/// exercises pure timer-driven preemption of ring-3 code, and needs
/// `kill(pid)` (or `sched::force_exit`) to ever end.
///
/// ```text
/// 1: jmp 1b
/// ```
pub const SPIN: &[u8] = &[0xeb, 0xfe];

/// `badptr`: `write`s through a kernel-half pointer, expects `-EFAULT`
/// back in `rax`, and reports it as its own exit code.
///
/// ```text
/// mov rax, 1                       ; write
/// mov rdi, 1                       ; fd 1
/// movabs rsi, 0xffffffff80000000   ; kernel image base (D15) -- never USER
/// mov rdx, 8
/// syscall
/// mov rdi, rax                     ; exit code = -EFAULT
/// mov rax, 0                       ; exit
/// syscall
/// 1: jmp 1b                        ; unreachable safety net
/// ```
pub const BADPTR: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x01, 0x00, 0x00, 0x00, // mov rax, 1
    0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00, // mov rdi, 1
    0x48, 0xbe, 0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff, // movabs rsi, 0xffffffff80000000
    0x48, 0xc7, 0xc2, 0x08, 0x00, 0x00, 0x00, // mov rdx, 8
    0x0f, 0x05, // syscall
    0x48, 0x89, 0xc7, // mov rdi, rax
    0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
    0x0f, 0x05, // syscall
    0xeb, 0xfe, // 1: jmp 1b
];

/// `growstack`: touches memory 1 MiB below its initial `rsp` (well below
/// the 64 KiB mapped up front, but inside the 16 MiB reservation) then
/// exits 0 -- exercises demand-grown stacks.
///
/// ```text
/// mov rax, rsp
/// sub rax, 0x100000                ; 1 MiB
/// mov byte ptr [rax], 1            ; touch it
/// xor rdi, rdi                     ; exit code 0
/// mov rax, 0                       ; exit
/// syscall
/// 1: jmp 1b                        ; unreachable safety net
/// ```
pub const GROWSTACK: &[u8] = &[
    0x48, 0x89, 0xe0, // mov rax, rsp
    0x48, 0x2d, 0x00, 0x00, 0x10, 0x00, // sub rax, 0x100000
    0xc6, 0x00, 0x01, // mov byte ptr [rax], 1
    0x48, 0x31, 0xff, // xor rdi, rdi
    0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
    0x0f, 0x05, // syscall
    0xeb, 0xfe, // 1: jmp 1b
];

/// `nosys`: calls the unassigned syscall number 999, expects `-ENOSYS`
/// back in `rax`, and exits with `-result` (i.e. `ENOSYS` itself).
///
/// ```text
/// mov rax, 999
/// syscall
/// neg rax                          ; -(-ENOSYS) = ENOSYS
/// mov rdi, rax
/// mov rax, 0                       ; exit
/// syscall
/// 1: jmp 1b                        ; unreachable safety net
/// ```
pub const NOSYS: &[u8] = &[
    0x48, 0xc7, 0xc0, 0xe7, 0x03, 0x00, 0x00, // mov rax, 999
    0x0f, 0x05, // syscall
    0x48, 0xf7, 0xd8, // neg rax
    0x48, 0x89, 0xc7, // mov rdi, rax
    0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
    0x0f, 0x05, // syscall
    0xeb, 0xfe, // 1: jmp 1b
];

// --- kernel-review round 2: adversarial payloads ---------------------------

/// `unmap_kernel_half`: `unmap`s a kernel-half address -- expects `-EINVAL`,
/// reports it as its exit code.
///
/// ```text
/// mov rax, 7                       ; unmap
/// movabs rdi, 0xffffffff80000000   ; kernel image base (D15)
/// mov rsi, 0x1000
/// syscall
/// neg rax                          ; -(-EINVAL) = EINVAL
/// mov rdi, rax
/// mov rax, 0                       ; exit
/// syscall
/// 1: jmp 1b
/// ```
pub const UNMAP_KERNEL_HALF: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x07, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x00, 0x00, 0x00, 0x80, 0xff, 0xff, 0xff, 0xff, 0x48, 0xc7,
    0xc6, 0x00, 0x10, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xf7, 0xd8, 0x48, 0x89, 0xc7, 0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00,
    0x00, 0x0f, 0x05, 0xeb, 0xfe,
];

/// `unmap_unmapped`: `unmap`s a page that's never been mapped -- expects
/// `-EINVAL`.
///
/// ```text
/// mov rax, 7        ; unmap
/// mov rdi, 0x500000 ; unmapped user address
/// mov rsi, 0x1000
/// syscall
/// neg rax
/// mov rdi, rax
/// mov rax, 0        ; exit
/// syscall
/// 1: jmp 1b
/// ```
pub const UNMAP_UNMAPPED: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x07, 0x00, 0x00, 0x00, 0x48, 0xc7, 0xc7, 0x00, 0x00, 0x50, 0x00, 0x48, 0xc7, 0xc6, 0x00, 0x10,
    0x00, 0x00, 0x0f, 0x05, 0x48, 0xf7, 0xd8, 0x48, 0x89, 0xc7, 0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x05,
    0xeb, 0xfe,
];

/// `read_own_code`: `read(0, 0x400000, 1)` -- the destination is the
/// process's own code page, mapped read+execute only (never writable), so
/// this must come back `-EFAULT` immediately (never hang waiting for a
/// keypress -- `syscall::table::sys_read` validates before blocking).
///
/// ```text
/// mov rax, 2          ; read
/// xor rdi, rdi        ; fd 0
/// mov rsi, 0x400000   ; own code page (read+exec only, never writable)
/// mov rdx, 1
/// syscall
/// neg rax
/// mov rdi, rax
/// mov rax, 0          ; exit
/// syscall
/// 1: jmp 1b
/// ```
pub const READ_OWN_CODE: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x02, 0x00, 0x00, 0x00, 0x48, 0x31, 0xff, 0x48, 0xc7, 0xc6, 0x00, 0x00, 0x40, 0x00, 0x48, 0xc7,
    0xc2, 0x01, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xf7, 0xd8, 0x48, 0x89, 0xc7, 0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00,
    0x00, 0x0f, 0x05, 0xeb, 0xfe,
];

/// `read_unmapped`: `read(0, 0x500000, 1)` into a never-mapped page --
/// expects `-EFAULT` immediately.
///
/// ```text
/// mov rax, 2
/// xor rdi, rdi
/// mov rsi, 0x500000   ; unmapped user address
/// mov rdx, 1
/// syscall
/// neg rax
/// mov rdi, rax
/// mov rax, 0          ; exit
/// syscall
/// 1: jmp 1b
/// ```
pub const READ_UNMAPPED: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x02, 0x00, 0x00, 0x00, 0x48, 0x31, 0xff, 0x48, 0xc7, 0xc6, 0x00, 0x00, 0x50, 0x00, 0x48, 0xc7,
    0xc2, 0x01, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xf7, 0xd8, 0x48, 0x89, 0xc7, 0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00,
    0x00, 0x0f, 0x05, 0xeb, 0xfe,
];

/// `map_anon_huge`: `map_anon(4 GiB)` -- exceeds the 1 GiB per-call cap;
/// expects `-EINVAL` (or `-ENOMEM`).
///
/// ```text
/// mov rax, 6                  ; map_anon
/// movabs rdi, 0x100000000     ; 4 GiB
/// syscall
/// neg rax
/// mov rdi, rax
/// mov rax, 0                  ; exit
/// syscall
/// 1: jmp 1b
/// ```
pub const MAP_ANON_HUGE: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x06, 0x00, 0x00, 0x00, 0x48, 0xbf, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x0f, 0x05,
    0x48, 0xf7, 0xd8, 0x48, 0x89, 0xc7, 0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xeb, 0xfe,
];

/// `map_anon_wua`: `map_anon` a page, write to it, `unmap` it, then touch it
/// again -- the last access must fault (the page is really gone) and kill
/// the process; the kernel must stay alive.
///
/// ```text
/// mov rax, 6              ; map_anon(0x1000)
/// mov rdi, 0x1000
/// syscall
/// mov rbx, rax             ; save the address (callee-saved, survives syscalls)
/// mov byte ptr [rbx], 0x42 ; write -- should succeed
/// mov rax, 7               ; unmap(rbx, 0x1000)
/// mov rdi, rbx
/// mov rsi, 0x1000
/// syscall
/// mov al, [rbx]             ; access again -- should fault
/// 1: jmp 1b                 ; unreachable
/// ```
pub const MAP_ANON_WUA: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x06, 0x00, 0x00, 0x00, 0x48, 0xc7, 0xc7, 0x00, 0x10, 0x00, 0x00, 0x0f, 0x05, 0x48, 0x89, 0xc3,
    0xc6, 0x03, 0x42, 0x48, 0xc7, 0xc0, 0x07, 0x00, 0x00, 0x00, 0x48, 0x89, 0xdf, 0x48, 0xc7, 0xc6, 0x00, 0x10, 0x00,
    0x00, 0x0f, 0x05, 0x8a, 0x03, 0xeb, 0xfe,
];

/// `nt_spin`: sets `RFLAGS.NT` via `pushf`/`or`/`popf`, then spins across
/// however many preemptions the test lets it have. Must never be able to
/// corrupt or crash the kernel (RFLAGS sanitisation on every return to ring
/// 3, brief kernel-review round 2) -- the process either keeps running
/// (until explicitly killed) or is killed by some fault; either is fine, as
/// long as the kernel itself stays alive.
///
/// ```text
/// pushfq
/// or qword ptr [rsp], 0x4000   ; NT (bit 14)
/// popfq
/// 1: jmp 1b
/// ```
pub const NT_SPIN: &[u8] = &[0x9c, 0x48, 0x81, 0x0c, 0x24, 0x00, 0x40, 0x00, 0x00, 0x9d, 0xeb, 0xfe];

/// `fpu_write`: sets `xmm0` to a distinctive bit pattern, yields twice (so
/// a real context switch saves it), then exits -- paired with `fpu_read`
/// to prove FPU state doesn't leak between processes.
///
/// ```text
/// movabs rax, 0x4141414141414141
/// movq xmm0, rax
/// mov rax, 4     ; yield
/// syscall
/// mov rax, 4     ; yield
/// syscall
/// xor rdi, rdi
/// mov rax, 0     ; exit
/// syscall
/// 1: jmp 1b
/// ```
pub const FPU_WRITE: &[u8] = &[
    0x48, 0xb8, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x66, 0x48, 0x0f, 0x6e, 0xc0, 0x48, 0xc7, 0xc0, 0x04,
    0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0xc7, 0xc0, 0x04, 0x00, 0x00, 0x00, 0x0f, 0x05, 0x48, 0x31, 0xff, 0x48, 0xc7,
    0xc0, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xeb, 0xfe,
];

/// `fpu_read`: reads `xmm0`'s low 64 bits into its own exit code -- `0` if
/// it's zero (correctly isolated from any other process, e.g. `fpu_write`),
/// `1` if it observed a nonzero (leaked) value.
///
/// ```text
/// movq rax, xmm0
/// test rax, rax
/// jz 2f
/// mov rdi, 1
/// jmp 3f
/// 2: xor rdi, rdi
/// 3: mov rax, 0   ; exit
/// syscall
/// 1: jmp 1b
/// ```
pub const FPU_READ: &[u8] = &[
    0x66, 0x48, 0x0f, 0x7e, 0xc0, 0x48, 0x85, 0xc0, 0x74, 0x09, 0x48, 0xc7, 0xc7, 0x01, 0x00, 0x00, 0x00, 0xeb, 0x03,
    0x48, 0x31, 0xff, 0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xeb, 0xfe,
];

/// `read_block`: blocks forever in `read(0, .., 1)` (no key is ever pressed
/// in the test environment) -- exercises `kill`ing a `Blocked` process.
///
/// ```text
/// sub rsp, 16
/// mov rax, 2     ; read
/// xor rdi, rdi   ; fd 0
/// mov rsi, rsp
/// mov rdx, 1
/// syscall
/// 1: jmp 1b       ; unreachable (killed while blocked)
/// ```
pub const READ_BLOCK: &[u8] = &[
    0x48, 0x83, 0xec, 0x10, 0x48, 0xc7, 0xc0, 0x02, 0x00, 0x00, 0x00, 0x48, 0x31, 0xff, 0x48, 0x89, 0xe6, 0x48, 0xc7,
    0xc2, 0x01, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xeb, 0xfe,
];

/// `sleep_long`: sleeps for 60 real seconds -- exercises `kill`ing a
/// `Sleeping` process.
///
/// ```text
/// mov rax, 5        ; sleep_ms
/// mov rdi, 60000
/// syscall
/// 1: jmp 1b          ; unreachable (killed while sleeping)
/// ```
pub const SLEEP_LONG: &[u8] = &[0x48, 0xc7, 0xc0, 0x05, 0x00, 0x00, 0x00, 0x48, 0xc7, 0xc7, 0x60, 0xea, 0x00, 0x00, 0x0f, 0x05, 0xeb, 0xfe];

/// `spawn_argv_len_overflow` (kernel-review M2-T3 fix): calls `spawn` with
/// a one-entry argv table whose claimed length is `0xFFFFFFFFFFFFFFF0`
/// (16 short of `u64::MAX`) -- must come back `-E2BIG` (7), never panic
/// the kernel via an overflowing `+`/`resize`. Reports the negated raw
/// syscall result as its own exit code (the same "`mov rdi, rax`, then
/// `exit`" pattern `BADPTR` already uses).
///
/// ```text
/// mov rax, 9                        ; spawn
/// lea rdi, [rip + path_str]         ; path_ptr
/// mov rsi, 1                        ; path_len
/// lea rdx, [rip + argv_table]       ; argv_ptr
/// mov r10, 1                        ; argc
/// syscall
/// mov rdi, rax                      ; exit code = spawn's raw (negative) result
/// mov rax, 0                        ; exit
/// syscall
/// 1: jmp 1b                          ; unreachable safety net
/// path_str: .ascii "x"
/// .align 8
/// argv_table:
///     .quad argv_str                 ; ptr (patched below to its real runtime address)
///     .quad 0xFFFFFFFFFFFFFFF0        ; claimed len, hostile
/// argv_str: .ascii "y"
/// ```
pub const SPAWN_ARGV_LEN_OVERFLOW: &[u8] = &[
    0x48, 0xc7, 0xc0, 0x09, 0x00, 0x00, 0x00, // mov rax, 9
    0x48, 0x8d, 0x3d, 0x25, 0x00, 0x00, 0x00, // lea rdi, [rip + 0x25] -> path_str (0x33)
    0x48, 0xc7, 0xc6, 0x01, 0x00, 0x00, 0x00, // mov rsi, 1
    0x48, 0x8d, 0x15, 0x1c, 0x00, 0x00, 0x00, // lea rdx, [rip + 0x1c] -> argv_table (0x38)
    0x49, 0xc7, 0xc2, 0x01, 0x00, 0x00, 0x00, // mov r10, 1
    0x0f, 0x05, // syscall
    0x48, 0x89, 0xc7, // mov rdi, rax
    0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x00, // mov rax, 0
    0x0f, 0x05, // syscall
    0xeb, 0xfe, // 1: jmp 1b
    0x78, // path_str: "x"
    0x0f, 0x1f, 0x40, 0x00, // .align 8 padding (never executed)
    // argv_table.ptr = 0x0000000000400048 -- the runtime address of
    // `argv_str` below (payloads always load at the fixed 0x400000,
    // D18; argv_str sits at static offset 0x48 in this exact array).
    0x48, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    // argv_table.len = 0xFFFFFFFFFFFFFFF0 (hostile).
    0xf0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x79, // argv_str: "y"
];
