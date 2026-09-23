//! Per-vector exception/interrupt entry stubs.
//!
//! The CPU's own hardware trap frame has two different shapes: ten vectors
//! (8, 10, 11, 12, 13, 14, 17, 21, 29, 30; Intel SDM Vol. 3A 6.15) push a
//! 32-bit error code before `RIP`; the rest don't. `stubs::define_stub!`
//! (in the `stubs` submodule -- 256 near-identical one-liners, split out
//! purely to keep this file under the ~600-line guideline) normalizes
//! that: it emits one `#[unsafe(naked)] unsafe extern "C" fn` per vector
//! that pushes a dummy 0 error code when the CPU didn't supply one, then
//! always pushes the vector number and jumps to the shared `common_stub`
//! (defined here), which pushes the 15 general-purpose registers, calls
//! `trap_dispatch` with `rsp` (now a valid `*mut TrapFrame`, see trap.rs)
//! in `rdi`, pops the registers back, drops the vector+error_code pair,
//! and `iretq`s. Vectors 2/8/18 are the exception -- see the module docs
//! below and their own hand-written stubs, kept here rather than in
//! `stubs`, right next to the `common_stub_ist1`/`common_stub_ist2` tails
//! they route to.
//!
//! Push order matters: `common_stub` pushes `rax` first and `r15` last, so
//! the *last* value pushed -- `r15` -- ends up at the lowest address, i.e.
//! at `rsp` itself once `mov rdi, rsp` runs. That is exactly
//! `TrapFrame`'s field order (`r15` first, `rax` last; see trap.rs):
//! reinterpreting `rdi` as `*mut TrapFrame` is valid because the struct
//! layout mirrors the stack layout byte for byte.
//!
//! Alignment: the CPU aligns `rsp` to 16 before pushing its 5- or 6-word
//! frame; with our dummy error code both shapes leave `rsp` a multiple of
//! 16. The vector push (+8 bytes) makes it 8 mod 16; the 15 GPR pushes
//! (120 bytes, itself 8 mod 16) bring it back to a multiple of 16, so
//! `call {dispatch}` -- which itself pushes an 8-byte return address --
//! enters `trap_dispatch` with `rsp` at 8 mod 16, exactly the SysV ABI's
//! requirement at function entry.
//!
//! `swapgs` discipline (brief M2-T2): `common_stub` checks the CPU's own
//! hardware frame's `CS` -- sitting at `[rsp+24]` on entry, *before* any
//! GPR is pushed (vector(0)/error_code(8)/rip(16)/cs(24), matching
//! `TrapFrame`'s tail exactly) -- and `swapgs`s only if its RPL is 3, i.e.
//! only for a trap that interrupted ring 3 code. A trap that interrupted
//! ring 0 code needs no swap: `GS_BASE` already holds the per-CPU area
//! (`arch::x86_64::percpu`) in that case, because whatever ring-0 code was
//! running either never left it (an ordinary kernel thread) or already
//! swapped it in itself on its own way from ring 3 (`syscall_entry`, or an
//! *outer* invocation of this same check). The same `[rsp+24]` read (still
//! the untouched hardware `CS`, restored to that exact stack slot by the
//! 15 `pop`s in between) decides the matching swap back on the way out --
//! see `arch::x86_64::percpu`'s module docs for why this stays correct
//! even across a preemption that switches to a completely different
//! thread in between.
//!
//! # Why `NMI`/`#MC`/`#DF` can't trust `CS.RPL` (kernel-review round 2)
//! `common_stub`'s `CS.RPL` check is only sound because every vector that
//! reaches it can only ever be *taken* while `IF == 1`. `arch::x86_64::
//! syscall_entry`'s own module docs describe a narrow window, on both the
//! way into and the way out of a `syscall`, where `CS` already reads (or
//! still reads) as ring 0 while `GS_BASE` has not yet been (or has already
//! stopped being) the per-CPU area -- and `SFMASK`/this kernel's own
//! `cli`/`sti` pairing guarantee `IF == 0` for that entire window, so no
//! *maskable* interrupt or synchronously-CS-consistent exception can ever
//! land there. `NMI` and `#MC` are non-maskable by definition, and a `#DF`
//! can in principle be provoked at an arbitrary point too -- so all three
//! get their own tails (`common_stub_ist1` for `#DF`'s IST1,
//! `common_stub_ist2` for `NMI`/`#MC`'s shared IST2), which decide
//! `swapgs` by `rdmsr`-reading the *actual* `GS_BASE` and comparing it
//! against `percpu::PERCPU`'s own address -- a check that's correct
//! regardless of which window it lands in, since it asks "is `GS_BASE`
//! already right?" instead of inferring an answer from `CS`. The decision
//! is remembered (`percpu::DF_GS_DID_SWAP`/`NMI_MC_GS_DID_SWAP`, plain
//! RIP-relative statics -- never `gs:`-relative, since that's precisely
//! what might not be safe to dereference through yet) so the matching
//! exit does the same action instead of re-deriving a stale one (by exit
//! time, the entry logic has *already* made `GS_BASE` correct, so a fresh
//! `rdmsr`-based check there would always say "already kernel", wrongly,
//! for the case where a real swap-back is needed).
//!
//! 256 distinct functions (rather than one generic one) are what let each
//! stub embed its own vector number as an immediate; `stubs::define_stub!`
//! is invoked once per vector (`stubs.rs`) so the only thing that varies
//! line to line is the vector number and whether the CPU supplies an
//! error code.

use crate::arch::x86_64::trap::trap_dispatch;

/// Reads RFLAGS.IF (Intel SDM Vol. 1 3.4.3): whether interrupts are
/// currently enabled on this core. Shared by `without_interrupts` below
/// and `sync::IrqMutex`.
pub fn interrupts_enabled() -> bool {
    let rflags: u64;
    // SAFETY: `pushfq`/`pop` only reads the current RFLAGS into `rflags`;
    // it doesn't execute `popfq`, so it has no effect on CPU state beyond
    // the (balanced) stack traffic the compiler is already told about by
    // omitting `nostack`.
    unsafe {
        core::arch::asm!("pushfq", "pop {}", out(reg) rflags, options(preserves_flags));
    }
    rflags & (1 << 9) != 0 // RFLAGS.IF, Intel SDM Vol. 1 3.4.3
}

/// Disables interrupts (`cli`).
///
/// # Safety
/// Always valid to execute from ring 0. The caller is responsible for
/// eventually restoring the flag (directly, or via `without_interrupts`/
/// `sync::IrqMutex`) if any code downstream assumes interrupts come back
/// on afterward.
#[inline]
pub unsafe fn cli() {
    // SAFETY: forwarded from this function's own contract.
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };
}

/// Enables interrupts (`sti`).
///
/// # Safety
/// The IDT must already be installed and every vector that can plausibly
/// fire must already have a safe handler -- true from the end of
/// `crate::start_interrupts` onward, never before. Prefer
/// `without_interrupts`/`sync::IrqMutex` over calling this directly: an
/// unconditional `sti` can re-enable interrupts underneath an outer
/// critical section that needed them to stay off.
#[inline]
pub unsafe fn sti() {
    // SAFETY: forwarded from this function's own contract.
    unsafe { core::arch::asm!("sti", options(nomem, nostack, preserves_flags)) };
}

/// Runs `f` with interrupts disabled, then restores RFLAGS.IF to whatever
/// it was on entry -- not just unconditionally re-enabling it, so nested
/// calls (or a call made while interrupts were already off) don't turn
/// them on underneath an outer caller that needs them to stay off.
///
/// This is how any IRQ-context caller stays safe against a lock this same
/// core already holds: `spin::Mutex` isn't reentrant, so if an interrupt
/// fired while, say, the PMM's lock were held and the handler tried to
/// take it again, the interrupted code would deadlock against itself
/// forever -- the same hazard `serial::EmergencyWriter`'s docs describe
/// for `SERIAL1`. Brief M1-T5 gives `SERIAL1`/the PMM/the heap their own
/// dedicated lock type instead (`sync::IrqMutex`, which wraps exactly
/// this save/disable/restore dance around a single `spin::Mutex`); this
/// closure-taking form remains for a critical section that isn't a
/// single lock.
pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
    let was_enabled = interrupts_enabled();

    // SAFETY: disabling interrupts is always valid from ring 0.
    unsafe { cli() };

    let result = f();

    if was_enabled {
        // SAFETY: re-enabling interrupts is valid from ring 0; this only
        // runs when this call is the one that found them enabled (and
        // therefore the one that turned them off above), so it can't
        // re-enable them underneath an outer `without_interrupts` that
        // found them already disabled.
        unsafe { sti() };
    }

    result
}

/// The shared second half of almost every stub: saves the 15 GPRs, calls
/// `trap_dispatch`, restores them, discards the vector+error_code pair the
/// per-vector stub pushed, and returns from the interrupt. Vectors 2
/// (`NMI`), 8 (`#DF`) and 18 (`#MC`) do *not* use this -- see
/// `common_stub_ist1`/`common_stub_ist2` below and this module's own docs
/// for why those three need a different `swapgs` decision.
#[unsafe(naked)]
unsafe extern "C" fn common_stub() {
    core::arch::naked_asm!(
        // `swapgs` in (see the module docs) -- only for a trap that
        // interrupted ring 3 (`CS.RPL == 3` at `[rsp+24]`). Sound here
        // specifically because every vector reaching this stub can only
        // ever be taken with `IF == 1` (an ordinary maskable interrupt or
        // a synchronous CPU exception raised by ring-3-or-ring-0 code
        // actually executing) -- never inside the narrow, `IF == 0`
        // `swapgs`-adjacent window `syscall_entry.rs`'s module docs
        // describe, which only `NMI`/`#MC`/`#DF` can ever land in.
        "test qword ptr [rsp + 24], 3",
        "jz 1f",
        "swapgs",
        "1:",
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "call {dispatch}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        // `swapgs` back out, symmetric to the check on entry -- `[rsp+24]`
        // still holds the same, untouched `CS` (nothing between here and
        // there wrote to it). Ring-3-return RFLAGS sanitisation (kernel-
        // review round 2, mirroring `syscall_entry.rs`'s identical policy
        // for `sysretq`): only for the ring-3 branch -- a ring-0-resuming
        // `iretq` must restore that interrupted kernel code's own flags
        // (e.g. a deliberately-cleared `IF` around a critical section)
        // completely unmodified.
        "test qword ptr [rsp + 24], 3",
        "jz 2f",
        "and qword ptr [rsp + 32], {rflags_preserve}",
        "or qword ptr [rsp + 32], {rflags_force_set}",
        "swapgs",
        "2:",
        "add rsp, 16",
        "iretq",
        dispatch = sym trap_dispatch,
        rflags_preserve = const super::syscall_entry::RFLAGS_PRESERVE_MASK,
        rflags_force_set = const super::syscall_entry::RFLAGS_FORCE_SET_MASK,
    );
}

/// `#DF` (vector 8, IST1)'s own tail: identical to `common_stub` except
/// for *how* it decides `swapgs` -- see this module's own docs for why.
/// The entry decision is remembered in `DF_GS_DID_SWAP` (a plain,
/// RIP-relative-addressed static -- *not* `gs:`-relative, since the whole
/// point is `GS_BASE` might not be safe to dereference through yet at that
/// exact point) so the exit does the *matching* action instead of
/// re-deriving a (by-then-always-"already kernel") answer from `GS_BASE`
/// a second time.
#[unsafe(naked)]
unsafe extern "C" fn common_stub_ist1() {
    core::arch::naked_asm!(
        "mov ecx, 0xc0000101", // GS_BASE_MSR
        "rdmsr",
        "shl rdx, 32",
        "or rax, rdx", // rax = current GS_BASE
        "lea rdx, [rip + {percpu}]",
        "cmp rax, rdx",
        "je 1f",
        "swapgs",
        "mov qword ptr [rip + {did_swap}], 1",
        "jmp 2f",
        "1:",
        "mov qword ptr [rip + {did_swap}], 0",
        "2:",
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "call {dispatch}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "cmp qword ptr [rip + {did_swap}], 0",
        "je 3f",
        "and qword ptr [rsp + 32], {rflags_preserve}",
        "or qword ptr [rsp + 32], {rflags_force_set}",
        "swapgs",
        "3:",
        "add rsp, 16",
        "iretq",
        dispatch = sym trap_dispatch,
        percpu = sym crate::arch::x86_64::percpu::PERCPU,
        did_swap = sym crate::arch::x86_64::percpu::DF_GS_DID_SWAP,
        rflags_preserve = const super::syscall_entry::RFLAGS_PRESERVE_MASK,
        rflags_force_set = const super::syscall_entry::RFLAGS_FORCE_SET_MASK,
    );
}

/// `NMI` (vector 2) and `#MC` (vector 18) share IST2, and this tail --
/// `arch::x86_64::trap`'s own nested-fault guard (`ist_guard_for`) already
/// treats the two as mutually exclusive (same IST stack, so a genuine
/// overlap would corrupt it anyway) -- so they safely share one
/// `did_swap` scratch flag too. Otherwise identical to `common_stub_ist1`;
/// see its own docs.
#[unsafe(naked)]
unsafe extern "C" fn common_stub_ist2() {
    core::arch::naked_asm!(
        "mov ecx, 0xc0000101", // GS_BASE_MSR
        "rdmsr",
        "shl rdx, 32",
        "or rax, rdx", // rax = current GS_BASE
        "lea rdx, [rip + {percpu}]",
        "cmp rax, rdx",
        "je 1f",
        "swapgs",
        "mov qword ptr [rip + {did_swap}], 1",
        "jmp 2f",
        "1:",
        "mov qword ptr [rip + {did_swap}], 0",
        "2:",
        "push rax",
        "push rcx",
        "push rdx",
        "push rbx",
        "push rbp",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov rdi, rsp",
        "call {dispatch}",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rbp",
        "pop rbx",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "cmp qword ptr [rip + {did_swap}], 0",
        "je 3f",
        "and qword ptr [rsp + 32], {rflags_preserve}",
        "or qword ptr [rsp + 32], {rflags_force_set}",
        "swapgs",
        "3:",
        "add rsp, 16",
        "iretq",
        dispatch = sym trap_dispatch,
        percpu = sym crate::arch::x86_64::percpu::PERCPU,
        did_swap = sym crate::arch::x86_64::percpu::NMI_MC_GS_DID_SWAP,
        rflags_preserve = const super::syscall_entry::RFLAGS_PRESERVE_MASK,
        rflags_force_set = const super::syscall_entry::RFLAGS_FORCE_SET_MASK,
    );
}

/// `#DF`/`NMI`/`#MC` (vectors 8/2/18) are hand-written here instead of
/// going through `stubs::define_stub!`: they route to `common_stub_ist1`/
/// `common_stub_ist2`, not the plain `common_stub` every other vector
/// shares -- see this module's own docs above. Every other vector's stub
/// (256 near-identical one-liners, no interesting logic) lives in the
/// `stubs` submodule purely to keep this file under the ~600-line
/// guideline; `stubs::STUBS` (re-exported below) is what `idt::init`
/// actually installs.
#[unsafe(naked)]
unsafe extern "C" fn stub_2() {
    core::arch::naked_asm!("push 0", "push 2", "jmp {common}", common = sym common_stub_ist2);
}

#[unsafe(naked)]
unsafe extern "C" fn stub_8() {
    core::arch::naked_asm!("push 8", "jmp {common}", common = sym common_stub_ist1);
}

#[unsafe(naked)]
unsafe extern "C" fn stub_18() {
    core::arch::naked_asm!("push 0", "push 18", "jmp {common}", common = sym common_stub_ist2);
}

mod stubs;
pub(crate) use stubs::STUBS;
