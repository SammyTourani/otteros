//! The raw kernel-thread stack switch (brief M2-T1): swaps `rsp` between
//! two kernel stacks, saving/restoring only the six callee-saved GPRs the
//! System V ABI requires a *function call* to preserve -- because that's
//! exactly what this is, from the compiler's point of view, on both
//! ends. `sched::schedule` calls this to switch away from the current
//! thread; a brand-new thread's very first `ret` off its own (pre-seeded,
//! see `sched::context::build_initial_stack`) stack lands in
//! `sched::context::trampoline` instead of back in `schedule`, which is
//! how a thread that has never run before gets started.
//!
//! Caller-saved registers need no attention here: `switch_to` is called
//! through an ordinary `unsafe extern "C" fn` call site, so the compiler
//! already assumes every caller-saved register can be clobbered across
//! it and reloads whatever it still needs afterward -- same as any other
//! function call. That is also why this is safe to leave interrupts
//! enabled or disabled across: nothing here inspects RFLAGS, and the
//! only memory this touches is the two stacks themselves.

/// Saves the current callee-saved registers and `rsp` into `*prev_rsp`,
/// then loads `next_rsp` and restores its callee-saved registers before
/// returning -- into whatever `next_rsp`'s stack has as its next return
/// address. For a thread that has run before, that is the instruction
/// right after its own previous call to `switch_to`; for a brand-new
/// thread, it is `sched::context::trampoline` (see
/// `sched::context::build_initial_stack`).
///
/// # Safety
/// - `prev_rsp` must be a valid, writable pointer (the calling thread's
///   own saved-`rsp` slot); nothing else may read or write through it
///   until this thread is switched back to.
/// - `next_rsp` must be a stack pointer this function itself previously
///   saved via some other thread's `prev_rsp` (and that thread must not
///   be running anywhere else), or a value `sched::context::
///   build_initial_stack` built for a thread that has never run yet.
/// - Interrupts must be disabled for the duration of this call (brief
///   M2-T1's design cautions): `sched::schedule`, the only caller, only
///   ever calls this with interrupts already off, and does not rely on
///   an interrupt landing mid-switch being handled any particular way.
/// - The caller must not be holding any lock whose guard's `Drop` needs
///   to run on this exact call stack before `switch_to` returns: this
///   call may not return to its immediate caller's own stack frame for
///   an arbitrarily long time (possibly never, if the outgoing thread is
///   exiting), and when it does "return", it does so on a *different*
///   occasion than the one that called it.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn switch_to(prev_rsp: *mut u64, next_rsp: u64) {
    core::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    );
}
