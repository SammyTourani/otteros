//! The `syscall`/`sysret` entry point (brief M2-T2, DECISIONS.md D16;
//! hardened per kernel-review round 2).
//!
//! # `swapgs` discipline (why this stays correct across preemption)
//! `syscall_entry` executes exactly one `swapgs` on entry and one on the
//! way back out -- the same "one pair per ring 3 <-> ring 0 crossing" rule
//! `arch::x86_64::usermode::enter_ring3` and the ring-3 half of
//! `arch::x86_64::interrupts::common_stub` also follow (see
//! `arch::x86_64::percpu`'s module docs for the invariant this maintains).
//! A timer interrupt landing *inside* this function (after the `sti`
//! below) finds `GS_BASE` already flipped to the per-CPU area -- correct,
//! since that interrupt is itself now ring-0-and-already-swapped -- so its
//! own `common_stub` sees `CS.RPL == 0` on ITS OWN trap frame (this
//! function's kernel-mode `CS`, not the interrupted user thread's) and
//! correctly skips *its* swapgs pair entirely. Whatever thread ends up
//! running next (via `sched::schedule`, possibly not this one, possibly
//! not even a user thread) never has to care that this function's own
//! `swapgs`-out hasn't happened yet: nothing but this exact call stack
//! ever un-swaps what this exact call stack swapped in.
//!
//! # The syscall-entry window (`CS` already ring 0, `GS` still ring 3's)
//! Between this function's first instruction (`swapgs`) and its second
//! (loading `rsp` from the now-correct `GS_BASE`), and symmetrically
//! between the last `swapgs` and `sysretq` on the way out, there is a
//! handful of instructions where `CS` already reads (or still reads, on
//! the way out) as ring 0 -- the `syscall` instruction itself changed `CS`
//! atomically -- while `GS_BASE` has *not yet* been (or has *already
//! stopped being*) the per-CPU area. An ordinary maskable interrupt can
//! never land in that window: `SFMASK` clears `IF` the instant `syscall`
//! executes, and this function's own `cli`/`sti` keep it clear for the
//! rest of the entry/exit sequences, so nothing maskable is ever pending
//! here. Only `NMI`/`#MC` (truly non-maskable) or a `#DF` that happens to
//! occur exactly then can -- and `interrupts.rs`'s dedicated stubs for
//! those three vectors know about this window explicitly (they decide
//! `swapgs` by comparing the *actual* `GS_BASE` MSR against the per-CPU
//! area's address, not by trusting `CS.RPL`, which would get it backwards
//! in exactly this window); see that file's module docs for the full
//! reasoning. Every *other* vector's stub can safely keep trusting
//! `CS.RPL`, precisely because it can only ever be entered when `IF` was
//! genuinely 1 -- which this window, by construction, never is.
//!
//! # Non-canonical `rcx` (CVE-2012-0217-style)
//! `sysret` loads `RIP` from `rcx` and switches `CPL` to 3 *before*
//! checking `rcx` is canonical -- a non-canonical value then faults
//! *after* the privilege change, at CPL 3, which historically let a
//! crafted `rcx` escalate privilege. `rcx` only ever holds whatever `rip`
//! was at the `syscall` instruction (always canonical, since the CPU was
//! already executing there) unless something along the way rewrites it --
//! nothing in this kernel does, but `syscall::syscall_dispatch` still
//! checks `is_canonical` below before ever letting this function's own
//! `call` site return, and kills the process outright on failure
//! (kernel-review round 2: *never* attempt a return -- by `sysretq` *or*
//! `iretq` -- to a non-canonical target; simpler, and strictly safer, than
//! trying to fault "safely" through `iretq`). By the time this function's
//! own post-`call` code ever runs, `rcx` is therefore already guaranteed
//! canonical.

use super::{cr, gdt, percpu};

/// Register state `syscall_entry` saves before calling `syscall::
/// syscall_dispatch`, in exactly the order it lands on the stack (lowest
/// address first, matching `arch::x86_64::trap::TrapFrame`'s identical
/// convention: the *last* value pushed ends up as the *first* field).
/// D16's argument registers (`rdi, rsi, rdx, r10, r8, r9`) and the
/// callee-saved GPRs (`rbx, rbp, r12..r15`) are both here -- the ABI only
/// strictly requires preserving the latter, but saving everything keeps
/// this symmetric with `TrapFrame` and leaves room for a future syscall
/// (or signal-like delivery) that needs to adjust a user register.
#[repr(C)]
pub struct SyscallFrame {
    pub user_rsp: u64,
    pub r11: u64,
    pub rcx: u64,
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r9: u64,
    pub r8: u64,
    pub r10: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rax: u64,
}

/// Whether `addr` is a canonical x86-64 address (Intel SDM Vol. 1 3.3.7.1:
/// bits 63:47 all equal) -- shared by `syscall::syscall_dispatch`'s
/// `rcx`-before-`sysretq` check (kernel-review round 2).
pub(crate) fn is_canonical(addr: u64) -> bool {
    ((addr as i64) << 16 >> 16) as u64 == addr
}

/// RFLAGS bits `syscall_entry`'s return path always preserves from the
/// value it's about to load into `r11` (kernel-review round 2's
/// sanitisation): the arithmetic flags, `DF` and `TF`. Every other bit --
/// in particular `NT`, `IOPL`, `RF`, `VM`, `AC`, `VIF`, `VIP` -- is forced
/// to `0` regardless of what the value being restored says, and `IF`/bit 1
/// are forced to `1` regardless (`FORCE_SET_MASK` below); see
/// `RFLAGS_SANITIZE_FORCE_MASK` in `interrupts.rs` for the identical
/// policy applied to a trap/IRQ's own return-to-ring-3 path. Allowing `TF`
/// through is why ring-3 `#DB` (vector 1) is one of `arch::x86_64::trap`'s
/// killable-fault vectors: a user thread that single-steps itself must
/// only ever be able to hurt itself.
///
/// `pub(crate)`, not private: `arch::x86_64::interrupts`'s `common_stub`
/// (and its `#DF`/`NMI`/`#MC` variants) apply this exact same policy to
/// their own iretq-to-ring-3 path.
pub(crate) const RFLAGS_PRESERVE_MASK: u64 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 10) | (1 << 11);
/// Bit 1 (always-1) and `IF` -- forced on regardless of the incoming value.
pub(crate) const RFLAGS_FORCE_SET_MASK: u64 = (1 << 1) | (1 << 9);

/// Enables `syscall`/`sysret` (`EFER.SCE`), points `LSTAR` at
/// `syscall_entry`, sets `STAR` so `syscall` lands in `CS 0x08 / SS 0x10`
/// and `sysret` returns to `CS 0x23 / SS 0x1b`, and sets `SFMASK` so
/// `syscall` clears `IF`/`TF`/`DF`/`AC`/`NT` on entry (D16, `NT` added per
/// kernel-review round 2 -- defence in depth alongside this function's own
/// explicit RFLAGS sanitisation on the way back out). Must run once, at
/// boot, after `arch::x86_64::percpu::init` (the per-CPU area
/// `syscall_entry` depends on must already exist) and before any user
/// thread can possibly execute a `syscall` instruction.
pub fn init() {
    // D16 / gdt.rs's own module docs: `STAR`'s low half is the `syscall`
    // base (`CS` directly, `SS` = that + 8); its high half is the
    // `sysret` base (`CS` = that + 16, `SS` = that + 8) -- `0x10`
    // (`KERNEL_DATA_SELECTOR`'s own value) is reused here purely because
    // `0x10 + 8 == USER_DATA_SELECTOR` and `0x10 + 16 == USER_CODE_SELECTOR`
    // for exactly this GDT layout (see gdt.rs's doc comment on
    // `USER_DATA_SELECTOR`), not because `sysret` has anything to do with
    // the *kernel* data selector.
    let star = (u64::from(gdt::KERNEL_DATA_SELECTOR) << 48) | (u64::from(gdt::KERNEL_CODE_SELECTOR) << 32);
    let lstar = syscall_entry as *const () as u64;
    const RFLAGS_TF: u64 = 1 << 8;
    const RFLAGS_IF: u64 = 1 << 9;
    const RFLAGS_DF: u64 = 1 << 10;
    const RFLAGS_AC: u64 = 1 << 18;
    const RFLAGS_NT: u64 = 1 << 14;
    let sfmask = RFLAGS_TF | RFLAGS_IF | RFLAGS_DF | RFLAGS_AC | RFLAGS_NT;

    // SAFETY: `EFER_SCE_BIT` only ever *enables* `syscall`/`sysret`, which
    // nothing before this point in boot executes; `STAR`/`LSTAR`/`SFMASK`
    // are the documented MSRs those instructions read, and `lstar` is a
    // real, `'static` function this same binary defines -- valid for as
    // long as the kernel runs.
    unsafe {
        let efer = cr::rdmsr(cr::EFER_MSR);
        cr::wrmsr(cr::EFER_MSR, efer | cr::EFER_SCE_BIT);
        cr::wrmsr(cr::STAR_MSR, star);
        cr::wrmsr(cr::LSTAR_MSR, lstar);
        cr::wrmsr(cr::SFMASK_MSR, sfmask);
    }
}

/// The `LSTAR` target: every `syscall` instruction anywhere in userspace
/// lands here, in ring 0, with `rcx` = the user `rip` to return to, `r11`
/// = the user `rflags` (already masked per `SFMASK`, before the CPU's own
/// automatic `AND`), and `rsp` still the *user* stack pointer (`syscall`,
/// unlike an interrupt gate, never switches stacks itself).
///
/// # Safety
/// Only ever reached via the `syscall` instruction from ring 3, per the
/// `LSTAR` MSR `init` programs -- never called directly.
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        // --- Entry: swapgs, switch to this thread's kernel stack -------
        "swapgs",
        "mov gs:[{user_rsp_scratch}], rsp",
        "mov rsp, gs:[{kernel_rsp}]",

        // --- Build the SyscallFrame (see its own field-order docs) ------
        "push rax",
        "push rdi",
        "push rsi",
        "push rdx",
        "push r10",
        "push r8",
        "push r9",
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "push rcx",
        "push r11",
        "push qword ptr gs:[{user_rsp_scratch}]",

        "mov rdi, rsp",
        "sti",
        // `dispatch` (`syscall::syscall_dispatch`) never returns here at
        // all if `exit`/`kill` ended this thread, or if the return `rcx`
        // it's about to hand back turned out non-canonical (see the
        // module docs) -- in both cases this whole naked function simply
        // never resumes past this `call`, which is exactly "never return
        // to user mode" (brief M2-T2 step 3) / "never attempt any return"
        // (kernel-review round 2).
        "call {dispatch}",
        "cli",

        // --- Restore every register -------------------------------------
        "pop qword ptr gs:[{user_rsp_scratch}]",
        "pop r11",
        // RFLAGS sanitisation (kernel-review round 2): force-clear every
        // bit outside `RFLAGS_PRESERVE_MASK`/`RFLAGS_FORCE_SET_MASK` in
        // the value `sysretq` is about to load from `r11` -- in
        // particular `NT` (which could otherwise turn a *later* `iretq`
        // this same thread's own kernel-mode code executes into a bogus
        // hardware task switch), `IOPL`/`VM`/`VIF`/`VIP`/`RF`/`AC`, no
        // matter what the user set via `popf` before its last syscall.
        "and r11, {rflags_preserve}",
        "or r11, {rflags_force_set}",
        "pop rcx",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "pop r9",
        "pop r8",
        "pop r10",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rax",

        // --- Return: rcx is guaranteed canonical by this point (the
        // module docs) -- `sysretq` unconditionally. -------------------
        "mov rsp, gs:[{user_rsp_scratch}]",
        "swapgs",
        "sysretq",

        user_rsp_scratch = const percpu::USER_RSP_SCRATCH_OFFSET,
        kernel_rsp = const percpu::KERNEL_RSP_OFFSET,
        rflags_preserve = const RFLAGS_PRESERVE_MASK,
        rflags_force_set = const RFLAGS_FORCE_SET_MASK,
        dispatch = sym crate::syscall::syscall_dispatch,
    );
}
