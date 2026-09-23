//! Per-thread user FPU/SSE state (DECISIONS.md D13, brief M2-T2 step 10).
//!
//! The kernel itself is compiled soft-float/no-SSE (D13) and never touches
//! XMM/YMM registers, so kernel code needs none of this. A thread that
//! *has* entered ring 3, though, gets a 512-byte, 16-byte-aligned FXSAVE
//! area (`sched::Thread::fpu_state`): saved on switch-out, restored on
//! switch-in, exactly like the callee-saved GPRs `arch::x86_64::switch`
//! already handles, just for a different register file.

use alloc::boxed::Box;
use core::sync::atomic::{AtomicBool, Ordering};

use super::cr;

/// CR0 bit 1 (Intel SDM Vol. 3A 2.5): "Monitor Coprocessor" -- required
/// (alongside `CR0_NE`) for `fxsave`/`fxrstor` to be legal at all.
const CR0_MP: u64 = 1 << 1;
/// CR0 bit 2: "Emulation" -- if set, every FPU/SSE instruction (including
/// `fxsave`/`fxrstor` themselves) raises `#NM` instead of executing.
/// Cleared explicitly (kernel-review round 2), not just assumed absent:
/// Limine/firmware make no promise about it either way.
const CR0_EM: u64 = 1 << 2;
/// CR0 bit 5: native (rather than legacy MS-DOS-compatible) x87 error
/// reporting -- the mode every modern OS (and `fxsave`/`fxrstor`) expects.
const CR0_NE: u64 = 1 << 5;
/// CR0 bit 3: "Task Switched" -- set by hardware task switches (never used
/// by this kernel, D4) to support *lazy* FPU context switching by making
/// the next FPU/SSE instruction fault; this kernel switches FPU state
/// eagerly (`sched::schedule`, unconditionally for a thread with
/// `fpu_state`), so this must stay clear or every SSE instruction in ring
/// 3 would `#NM` (kernel-review round 2: explicit, not assumed).
const CR0_TS: u64 = 1 << 3;
/// CR4 bit 9: "Operating System support for FXSAVE/FXRSTOR" -- without
/// this, `fxsave`/`fxrstor` themselves (and any SSE instruction) raise
/// `#UD`.
const CR4_OSFXSR: u64 = 1 << 9;
/// CR4 bit 10: lets an unmasked SIMD floating-point exception raise `#XM`
/// instead of silently behaving as if masked.
const CR4_OSXMMEXCPT: u64 = 1 << 10;
/// The architectural power-on/reset value of MXCSR (Intel SDM Vol. 1
/// 11.6.4): all six SIMD exceptions masked, round-to-nearest, no flush-
/// to-zero/denormals-are-zero. `init` loads this explicitly rather than
/// trusting whatever the hardware already happened to hold.
const MXCSR_DEFAULT: u32 = 0x1F80;

/// A 512-byte, 16-byte-aligned FXSAVE/FXRSTOR image (Intel SDM Vol. 1
/// 10.5.1) -- the exact size and alignment both instructions require.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FxsaveArea(pub(crate) [u8; 512]);

impl FxsaveArea {
    /// A fresh area holding a copy of the "pristine" state `init` captured
    /// at boot -- what a brand-new user thread's FPU/SSE registers should
    /// look like before it has ever run an instruction.
    ///
    /// # Panics
    /// If `init` hasn't run yet.
    pub fn pristine() -> Box<Self> {
        assert!(READY.load(Ordering::Acquire), "fpu::FxsaveArea::pristine: fpu::init was never called");
        // SAFETY: `PRISTINE` was fully written by `init` before `READY` was
        // set (see its own ordering), and is never mutated again -- a
        // plain, `Copy` read out of a `'static`, already-initialised value.
        let copy = unsafe { PRISTINE };
        Box::new(copy)
    }
}

/// The pristine FXSAVE image `init` captures once, at boot, by actually
/// executing `fxsave` right after enabling the CR0/CR4 bits that make it
/// legal -- rather than hand-building a 512-byte bit pattern ourselves,
/// which risks a reserved/invalid field `fxrstor` would refuse (`#GP`) the
/// first time some user thread's initial state is ever loaded. Its type is
/// `FxsaveArea` itself (not a bare `[u8; 512]`) specifically so the
/// `align(16)` `fxsave` needs is enforced on this static the same way it
/// is on every heap-allocated one.
static mut PRISTINE: FxsaveArea = FxsaveArea([0; 512]);
static READY: AtomicBool = AtomicBool::new(false);

/// Enables the CR0/CR4 bits `fxsave`/`fxrstor` (and SSE in general) need,
/// explicitly clearing `CR0.EM`/`CR0.TS` (kernel-review round 2: never
/// just assumed already clear), then captures a known-clean, explicitly-
/// initialised FPU/SSE state as `PRISTINE` -- `fninit` (x87 defaults) plus
/// the architectural default `MXCSR` (kernel-review round 2: not
/// "whatever the hardware happened to reset to", so no process can ever
/// observe another's leftover register contents through a `pristine()`
/// area that wasn't genuinely blank). Must run once, at boot, before any
/// user thread is ever created (`sched::Thread::new_user_ready` calls
/// `FxsaveArea::pristine`).
pub fn init() {
    // SAFETY: setting `MP`/`NE`/`OSFXSR`/`OSXMMEXCPT` and clearing `EM`/`TS`
    // only ever *enables* functionality (native x87 error reporting,
    // FXSAVE/FXRSTOR, unmasked-SIMD-exception delivery, and FPU/SSE
    // instructions not faulting) that nothing before this point could have
    // been relying on being absent -- the kernel itself never executes an
    // FPU/SSE instruction (D13), so there is no existing behaviour these
    // bits could change out from under.
    unsafe {
        cr::write_cr0((cr::read_cr0() | CR0_MP | CR0_NE) & !(CR0_EM | CR0_TS));
        cr::write_cr4(cr::read_cr4() | CR4_OSFXSR | CR4_OSXMMEXCPT);
    }

    // SAFETY: `fninit` and `ldmxcsr` are both unprivileged and, as of the
    // writes just above, legal (CR0.EM/TS clear, CR4.OSFXSR set); `mxcsr`
    // is a local on this function's own stack, valid for the duration of
    // the `ldmxcsr` that reads it.
    unsafe {
        core::arch::asm!("fninit", options(nomem, nostack));
        let mxcsr = MXCSR_DEFAULT;
        core::arch::asm!("ldmxcsr [{0}]", in(reg) &mxcsr, options(readonly, nostack));
    }

    let ptr = (&raw mut PRISTINE).cast::<u8>();
    // SAFETY: `ptr` is `PRISTINE`'s own `'static`, `align(16)` (its type's
    // own `#[repr(align(16))]`) address, exclusively ours this early in
    // boot (single-threaded, nothing else has a reference to it yet).
    unsafe { core::arch::asm!("fxsave [{0}]", in(reg) ptr, options(nostack)) };

    READY.store(true, Ordering::Release);
}

/// Saves the current FPU/SSE register state into `area` (`sched::schedule`,
/// switching a thread with `Some(fpu_state)` *out*).
pub fn save(area: &mut FxsaveArea) {
    // SAFETY: `area.0` is a `#[repr(C, align(16))]`, 512-byte buffer --
    // exactly what `fxsave` requires of its memory operand.
    unsafe { core::arch::asm!("fxsave [{0}]", in(reg) area.0.as_mut_ptr(), options(nostack)) };
}

/// Restores the FPU/SSE register state from `area` (`sched::schedule`,
/// switching a thread with `Some(fpu_state)` *in*).
pub fn restore(area: &FxsaveArea) {
    // SAFETY: `area.0` was either produced by a prior `save` into this
    // exact same, still-live `FxsaveArea`, or is an unmodified copy of
    // `PRISTINE` (`FxsaveArea::pristine`) -- both are valid FXSAVE images
    // for `fxrstor` to load, at the required 16-byte alignment.
    unsafe { core::arch::asm!("fxrstor [{0}]", in(reg) area.0.as_ptr(), options(nostack)) };
}
