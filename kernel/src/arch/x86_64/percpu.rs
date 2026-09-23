//! The per-CPU area (brief M2-T2): a single, static `PerCpu` struct
//! reached through `KERNEL_GS_BASE`/`swapgs`.
//!
//! Invariant this whole kernel's `swapgs` discipline relies on: `GS_BASE`
//! holds this struct's address *whenever ring 0 code is executing*, and
//! `KERNEL_GS_BASE` holds 0 (this kernel gives userspace no per-thread
//! GS/TLS yet, so "the user's own GS_BASE" is always 0 for every thread --
//! D25/SMP and any future per-thread GS state will need to make that
//! explicit per-thread state instead of a bare global, see the TODO on
//! `init` below) whenever ring 3 code is executing. Every ring 3 <-> ring
//! 0 crossing (`arch::x86_64::syscall_entry`'s `syscall_entry`,
//! `arch::x86_64::usermode::enter_ring3`, and the ring-3 half of
//! `arch::x86_64::interrupts::common_stub`) does exactly one `swapgs` each
//! way, so the pair as a whole always holds `{PERCPU's address, 0}` in
//! some order -- see `syscall_entry.rs`'s module docs for the full
//! reasoning about why this stays correct across preemption/scheduling.
//!
//! Single-core only (M2; D25's multicore work needs one of these per CPU,
//! each with its own `GS_BASE`).

use core::mem::offset_of;

use super::cr;
use crate::sched::ThreadId;

#[repr(C)]
pub(crate) struct PerCpu {
    /// The current thread's kernel stack top. `syscall_entry`'s naked asm
    /// loads `rsp` from here on every `syscall` (unlike an interrupt gate,
    /// `syscall` never consults the TSS's `RSP0`) -- kept in sync with
    /// `gdt::set_rsp0` by `sched::schedule` on every context switch.
    kernel_rsp: u64,
    /// Scratch slot `syscall_entry` stashes the caller's `rsp` in, between
    /// swapping onto the kernel stack and pushing it into the
    /// `SyscallFrame` (and back out again on the way out).
    user_rsp_scratch: u64,
    /// The currently running thread's id -- informational only (tests,
    /// debugging); no asm path ever reads it back.
    current_thread: u64,
}

pub const KERNEL_RSP_OFFSET: usize = offset_of!(PerCpu, kernel_rsp);
pub const USER_RSP_SCRATCH_OFFSET: usize = offset_of!(PerCpu, user_rsp_scratch);

// `static mut`, not an `IrqMutex`/`UnsafeCell`-wrapped `static` (kernel-
// review precedent: `gdt::TSS`): every field here is touched either by
// `syscall_entry`'s naked asm (which can't take a lock) or by
// `sched::schedule` with interrupts already disabled, single-core, never
// concurrently with anything else that reads it meaningfully. Every write
// below goes through a raw pointer (`&raw mut`), never an intermediate
// `&mut` to the `static mut` itself, matching `gdt.rs`'s identical pattern.
//
// `pub(crate)`, not private: `arch::x86_64::interrupts`'s `#DF`/`NMI`/`#MC`
// stubs (kernel-review round 2) reference this directly via a `sym`
// operand, to compare the *actual* `GS_BASE` MSR against its address --
// see that module's docs for why those three specifically can't just
// trust `CS.RPL` the way every other vector's stub does.
pub(crate) static mut PERCPU: PerCpu = PerCpu { kernel_rsp: 0, user_rsp_scratch: 0, current_thread: 0 };

/// Scratch flags remembering whether the `#DF` (IST1) / `NMI`+`#MC` (IST2,
/// shared -- `arch::x86_64::trap`'s own nested-fault guard already treats
/// the two as mutually exclusive, since they share a stack) entry paths
/// had to `swapgs` on the way in, so the matching exit knows whether to
/// swap back (kernel-review round 2). Deliberately plain, RIP-relative-
/// addressable statics -- *never* `gs:`-relative -- since the whole point
/// of these three vectors' own entry check is that `GS_BASE` might not
/// even be the right thing to dereference through yet at that point; see
/// `interrupts.rs`'s module docs.
pub(crate) static mut DF_GS_DID_SWAP: u64 = 0;
pub(crate) static mut NMI_MC_GS_DID_SWAP: u64 = 0;

/// Points `GS_BASE` at the per-CPU area and `KERNEL_GS_BASE` at 0 (see the
/// module docs' invariant) -- must run once, at boot, before any ring 3
/// code can possibly exist to observe either MSR, and after `gdt::init`
/// (which loads a null selector into `gs`, a `mov` that -- unlike
/// `swapgs` -- reloads the *hidden* base from the, here always zero,
/// descriptor table entry, so this must come after it to have the last
/// word).
///
/// TODO(D25/SMP, per-thread TLS): once user threads get their own GS-based
/// TLS, `KERNEL_GS_BASE`'s "0 for everyone" assumption above needs to
/// become real per-thread state saved/restored at context switch (like
/// `Thread`'s FPU area), not a single global constant.
pub fn init() {
    let addr = &raw const PERCPU as u64;
    // SAFETY: `GS_BASE_MSR`/`KERNEL_GS_BASE_MSR` exist on every CPU that
    // implements `syscall`/`sysret` (every x86_64 chip); `addr` is
    // `PERCPU`'s own `'static` address, valid forever. Boot-time, single-
    // threaded: nothing yet relies on either MSR's previous value.
    unsafe {
        cr::wrmsr(cr::GS_BASE_MSR, addr);
        cr::wrmsr(cr::KERNEL_GS_BASE_MSR, 0);
    }
}

/// Updates the per-CPU area's `kernel_rsp` -- `sched::schedule` calls this
/// alongside `gdt::set_rsp0` on every context switch, so both the
/// hardware (TSS `RSP0`, for interrupts/exceptions) and `syscall_entry`'s
/// own manual stack switch always agree on "the current thread's kernel
/// stack top".
pub fn set_kernel_rsp(top: u64) {
    let ptr = &raw mut PERCPU;
    // SAFETY: see the `static mut PERCPU` docs above.
    unsafe { (*ptr).kernel_rsp = top };
}

/// Records the currently running thread's id (informational; see the
/// field's own docs).
pub fn set_current_thread(id: ThreadId) {
    let ptr = &raw mut PERCPU;
    // SAFETY: see the `static mut PERCPU` docs above.
    unsafe { (*ptr).current_thread = id };
}

/// Test-only introspection: the per-CPU area's current `kernel_rsp`.
pub fn kernel_rsp() -> u64 {
    // SAFETY: a single, aligned `u64` read of the same `'static` struct
    // (a copy of one `Copy` field, not a reference to it -- see gdt.rs's
    // `rsp0()` for the identical pattern); see the `static mut PERCPU`
    // docs above for why this is safe despite no lock.
    unsafe { PERCPU.kernel_rsp }
}

/// Test-only introspection: the per-CPU area's current `current_thread`.
pub fn current_thread() -> ThreadId {
    // SAFETY: see `kernel_rsp`.
    unsafe { PERCPU.current_thread }
}
