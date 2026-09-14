//! `TrapFrame` and the shared exception/interrupt dispatcher every stub in
//! `interrupts.rs` calls through `common_stub`.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::arch::x86_64::{cr, irq};
use crate::kprintln_emergency;
use crate::mm::addr::VirtAddr;
use crate::mm::kstack;
use crate::sync::IrqMutex;

/// Register state captured on the stack by `interrupts::common_stub`, in
/// exactly the order it lands there (see that module's doc comment): the
/// 15 general-purpose registers (`r15` first, `rax` last), then the vector
/// number and error code the per-vector stub pushed, then the 5-word frame
/// the CPU itself pushes for every exception/interrupt in long mode.
///
/// `size_of::<TrapFrame>()` is `22 * 8 = 176` bytes; `trapframe_layout`
/// (test_main.rs) pins that down along with the offsets of `vector` and
/// `rip`.
#[repr(C)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

/// Counts breakpoints (`int3`) handled since boot; `breakpoint_returns` and
/// `breakpoint_twice` (test_main.rs) check it advances by exactly one/two.
static BREAKPOINT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The number of `int3` breakpoints dispatched so far.
pub fn breakpoint_count() -> usize {
    BREAKPOINT_COUNT.load(Ordering::SeqCst)
}

/// Snapshot of a few GPRs `trap_dispatch` decoded from the most recent
/// breakpoint's `TrapFrame`. Exists purely for `trapframe_canaries`
/// (test_main.rs): it loads distinct, known values into these registers,
/// `int3`s, and checks each one decoded into the *matching* field --
/// proving the stub's push order and `TrapFrame`'s field order agree
/// register-by-register, not just that the struct's overall size/a couple
/// of offsets happen to be right (`trapframe_layout` already covers that,
/// but wouldn't notice e.g. two GPRs swapped in the push order).
#[derive(Clone, Copy, Default)]
pub struct BreakpointRegs {
    pub rbx: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/// `IrqMutex` (kernel-review, M1-T5 fix #2), not a bare `spin::Mutex`:
/// this is written from the `#BP` handler below (which, like every trap,
/// runs with IF already cleared by the interrupt gate it came through)
/// but *read* by `last_breakpoint_regs` from normal code, which runs with
/// interrupts on -- a timer tick landing on this core while normal code
/// held this lock would otherwise be unable to safely re-enter `#BP`
/// (not a real hazard for THIS lock specifically, since ticks never take
/// a breakpoint) but, more importantly, matches the same "any lock a trap
/// handler touches must be IRQ-safe" invariant `sync::IrqMutex`'s own
/// docs establish for `SERIAL1`/the PMM/the heap, rather than leaving one
/// exception to it.
static LAST_BREAKPOINT_REGS: IrqMutex<BreakpointRegs> = IrqMutex::new(BreakpointRegs {
    rbx: 0,
    r12: 0,
    r13: 0,
    r14: 0,
    r15: 0,
});

/// The GPR snapshot from the most recently dispatched breakpoint.
pub fn last_breakpoint_regs() -> BreakpointRegs {
    *LAST_BREAKPOINT_REGS.lock()
}

/// One flag per IST-switching vector (gdt.rs: IST1 is #DF, IST2 is
/// NMI/#MC), set for the duration of that vector's handler and checked on
/// entry: if a handler for a given IST is entered while its own flag is
/// already set, the machine is faulting again *on the same stack* before
/// the first fault finished with it (or that stack is corrupt) -- there is
/// no third stack to fall back to, so `trap_dispatch` halts immediately
/// instead of trying (and likely failing the same way) to handle it
/// normally.
static IST1_IN_USE: AtomicBool = AtomicBool::new(false);
static IST2_IN_USE: AtomicBool = AtomicBool::new(false);

/// The re-entry guard flag for `vector`'s handler, and which IST number
/// it's guarding (for the log message), if `vector` uses an IST at all.
fn ist_guard_for(vector: u64) -> Option<(&'static AtomicBool, u8)> {
    match vector {
        8 => Some((&IST1_IN_USE, 1)),
        2 | 18 => Some((&IST2_IN_USE, 2)),
        _ => None,
    }
}

/// A second fault on the same IST stack before the first one cleared its
/// guard flag: print (lock-free -- `SERIAL1` may well be part of what's
/// broken) and halt for good. `qemu::exit` writes the isa-debug-exit
/// failure code first (so test mode gets a clean failure instead of a
/// timeout) and its own fallback, for when that device isn't there, is
/// exactly "halt forever" -- but `cli` first, unconditionally, rather than
/// trusting whatever left us here to have kept interrupts off.
fn nested_fault_halt(ist_n: u8) -> ! {
    kprintln_emergency!("EXCEPTION: nested fault on IST{ist_n}, halting");
    // SAFETY: disabling interrupts unconditionally before parking the CPU
    // (or asking QEMU to exit) for good is always valid from ring 0, and
    // this function never does anything else afterwards.
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };
    crate::qemu::exit(false);
}

/// Mnemonic for exception vectors 0-31 (Intel SDM Vol. 3A, table 6-1).
/// Vectors 32 and up aren't CPU exceptions (they become IRQs in a later
/// task), so they're not named here.
fn mnemonic(vector: u64) -> &'static str {
    match vector {
        0 => "DIVIDE ERROR",
        1 => "DEBUG",
        2 => "NON-MASKABLE INTERRUPT",
        3 => "BREAKPOINT",
        4 => "OVERFLOW",
        5 => "BOUND RANGE EXCEEDED",
        6 => "INVALID OPCODE",
        7 => "DEVICE NOT AVAILABLE",
        8 => "DOUBLE FAULT",
        9 => "COPROCESSOR SEGMENT OVERRUN",
        10 => "INVALID TSS",
        11 => "SEGMENT NOT PRESENT",
        12 => "STACK-SEGMENT FAULT",
        13 => "GENERAL PROTECTION FAULT",
        14 => "PAGE FAULT",
        16 => "X87 FLOATING-POINT EXCEPTION",
        17 => "ALIGNMENT CHECK",
        18 => "MACHINE CHECK",
        19 => "SIMD FLOATING-POINT EXCEPTION",
        20 => "VIRTUALIZATION EXCEPTION",
        21 => "CONTROL PROTECTION EXCEPTION",
        28 => "HYPERVISOR INJECTION EXCEPTION",
        29 => "VMM COMMUNICATION EXCEPTION",
        30 => "SECURITY EXCEPTION",
        _ => "RESERVED",
    }
}

/// Prints the full register dump every fatal exception path shares.
fn dump(frame: &TrapFrame) {
    kprintln_emergency!(
        "[trap] rax={:016x} rbx={:016x} rcx={:016x} rdx={:016x}",
        frame.rax,
        frame.rbx,
        frame.rcx,
        frame.rdx
    );
    kprintln_emergency!(
        "[trap] rsi={:016x} rdi={:016x} rbp={:016x} rsp={:016x}",
        frame.rsi,
        frame.rdi,
        frame.rbp,
        frame.rsp
    );
    kprintln_emergency!(
        "[trap] r8 ={:016x} r9 ={:016x} r10={:016x} r11={:016x}",
        frame.r8,
        frame.r9,
        frame.r10,
        frame.r11
    );
    kprintln_emergency!(
        "[trap] r12={:016x} r13={:016x} r14={:016x} r15={:016x}",
        frame.r12,
        frame.r13,
        frame.r14,
        frame.r15
    );
    kprintln_emergency!(
        "[trap] rip={:016x} cs={:04x} rflags={:016x} ss={:04x} vector={} error_code={:#x}",
        frame.rip,
        frame.cs,
        frame.rflags,
        frame.ss,
        frame.vector,
        frame.error_code
    );
}

/// Decodes a page-fault error code's low 5 bits (Intel SDM Vol. 3A 4.7)
/// into a short human-readable flag summary.
fn page_fault_flags(error_code: u64) -> (bool, bool, bool, bool, bool) {
    let present = error_code & 1 != 0;
    let write = error_code & (1 << 1) != 0;
    let user = error_code & (1 << 2) != 0;
    let reserved = error_code & (1 << 3) != 0;
    let ifetch = error_code & (1 << 4) != 0;
    (present, write, user, reserved, ifetch)
}

/// The single dispatch point every one of the 256 stubs in `interrupts.rs`
/// calls: `frame_ptr` points at a `TrapFrame` built on that stub's own
/// stack (either the interrupted stack, or the IST1/IST2 stack for
/// vectors 8, 2 and 18; see gdt.rs).
///
/// # Safety
/// `frame_ptr` must point at a live, fully-initialized `TrapFrame`; only
/// `interrupts::common_stub` ever calls this, immediately after building
/// exactly that.
pub unsafe extern "C" fn trap_dispatch(frame_ptr: *mut TrapFrame) {
    // Read the vector number through the raw pointer first, before
    // deciding which reference kind this call will ever form.
    // Kernel-review (M1-T5 fix #5) flagged an earlier version of this
    // function for forming both a `&TrapFrame` (`frame`, used by every
    // CPU-exception arm below) and, in the same call, a `&mut TrapFrame`
    // in the vector `>= 32` arm -- sound under NLL (the shared reference's
    // last *textual* use precedes the mutable one), but exactly the kind
    // of aliasing pattern that's easy to get wrong the next time this
    // function changes. Reading `vector` as a plain `u64` copy up front,
    // then branching *before* forming either reference, means each path
    // below forms exactly one reference to `*frame_ptr`, of exactly one
    // kind, and the shared reference the exception path builds is never
    // touched again once that path's own match returns.
    //
    // SAFETY: `frame_ptr` is valid per this function's own contract (only
    // `interrupts::common_stub` ever calls this, immediately after
    // building a full `TrapFrame` there); reading one `u64` field through
    // a raw pointer doesn't require forming a `&TrapFrame` first.
    let vector = unsafe { (*frame_ptr).vector };

    if vector >= 32 {
        // SAFETY: no `&TrapFrame` to `*frame_ptr` has been formed
        // anywhere in this call (the read above went through the raw
        // pointer directly), so this `&mut` is the only live reference to
        // it, and this function returns immediately after using it.
        // `irq::dispatch` calls whichever handler `irq::register`
        // installed for `vector` (or counts it spurious and still sends
        // an EOI, brief M1-T5 step 7) with a *mutable* reference, matching
        // `irq::Handler`'s signature -- nothing needs it yet, but a future
        // handler (a syscall-like trap, a signal-delivery path) might need
        // to adjust the interrupted context before `iretq`.
        let frame_mut = unsafe { &mut *frame_ptr };
        irq::dispatch(vector as u8, frame_mut);
        return;
    }

    // `vector` is now known (by this function's own control flow, not by
    // the type system) to be a CPU exception in `0..=31`: this is the one
    // shared reference the rest of this call ever forms, and no `&mut` to
    // the same memory is ever formed again before this function returns.
    //
    // SAFETY: forwarded from this function's own contract; `frame_ptr` is
    // still valid for reads, and only ever aliased (mutably) on the
    // early-return path above, which this control-flow path didn't take.
    let frame = unsafe { &*frame_ptr };

    // IST re-entry guard (kernel-review, M1-T1): vectors 8/2/18 run on a
    // dedicated IST stack precisely so they still have a good stack when
    // everything else is on fire. If one of them is entered while its own
    // flag is already set, that safety net has itself just failed --
    // there's nowhere left to go but a halt.
    let guard = ist_guard_for(frame.vector);
    if let Some((flag, ist_n)) = guard
        && flag.swap(true, Ordering::SeqCst)
    {
        nested_fault_halt(ist_n);
    }

    match frame.vector {
        3 => {
            BREAKPOINT_COUNT.fetch_add(1, Ordering::SeqCst);
            *LAST_BREAKPOINT_REGS.lock() = BreakpointRegs {
                rbx: frame.rbx,
                r12: frame.r12,
                r13: frame.r13,
                r14: frame.r14,
                r15: frame.r15,
            };
            kprintln_emergency!("[trap] breakpoint rip=0x{:x}", frame.rip);
        }
        14 => {
            let cr2 = cr::read_cr2();
            let (present, write, user, reserved, ifetch) = page_fault_flags(frame.error_code);
            kprintln_emergency!(
                "EXCEPTION: PAGE FAULT at 0x{:x} (present={} write={} user={} reserved={} ifetch={}) rip=0x{:x}",
                cr2,
                present,
                write,
                user,
                reserved,
                ifetch,
                frame.rip
            );
            dump(frame);
            panic!("page fault at {cr2:#x}");
        }
        8 => {
            // A kernel stack overflow reaches here (brief M1-T4 step 5)
            // because #PF (vector 14) uses no IST: once the recursion's
            // own writes cross into an unmapped guard page, the CPU's
            // attempt to push *that* page fault's own interrupt frame --
            // still at the same, now-invalid `rsp` -- faults again, and
            // page-fault-while-page-fault is one of the documented #DF
            // combinations (Intel SDM Vol. 3A Table 6-5).
            //
            // Kernel-review fix #3: `frame.rsp` (the CPU's own hardware
            // frame always records it, even for a same-privilege #DF) is
            // the *primary* signal -- it's exactly the stack pointer that
            // was already inside (or a few bytes above) the guard page
            // when the overflow happened, and unlike CR2 it can never be
            // stale (CR2 only reflects the *second*, nested #PF, and in
            // principle some other path could reach vector 8 with CR2
            // left over from something else entirely). CR2 is checked too
            // and reported only as corroboration, never as the sole basis
            // for the "stack overflow" diagnosis.
            let cr2 = cr::read_cr2();
            let rsp_guard = kstack::find_guard(VirtAddr::new(frame.rsp));
            let cr2_guard = kstack::find_guard(VirtAddr::new(cr2));
            match (rsp_guard, cr2_guard) {
                (Some(stack), corroboration) => kprintln_emergency!(
                    "EXCEPTION: DOUBLE FAULT -- kernel stack overflow (guard page 0x{:x}, signal=rsp{})",
                    stack.guard.as_u64(),
                    if corroboration.is_some() { "+cr2" } else { "" }
                ),
                (None, Some(stack)) => kprintln_emergency!(
                    "EXCEPTION: DOUBLE FAULT -- kernel stack overflow (guard page 0x{:x}, signal=cr2 only, rsp=0x{:x} did not match)",
                    stack.guard.as_u64(),
                    frame.rsp
                ),
                (None, None) => kprintln_emergency!("EXCEPTION: DOUBLE FAULT"),
            }
            dump(frame);
            if let Some((flag, _)) = guard {
                flag.store(false, Ordering::SeqCst);
            }
            panic!("double fault");
        }
        0..=31 => {
            kprintln_emergency!("EXCEPTION: {}", mnemonic(frame.vector));
            dump(frame);
            if let Some((flag, _)) = guard {
                flag.store(false, Ordering::SeqCst);
            }
            panic!("unhandled exception: {}", mnemonic(frame.vector));
        }
        _ => unreachable!(
            "trap_dispatch: vector {} should have taken the vector >= 32 early return above",
            frame.vector
        ),
    }

    // Normal-return path (breakpoint only -- vector >= 32 already
    // returned early above): clear the guard flag, if this vector had
    // one. Vector 3 never sets one (see `ist_guard_for`), so this is a
    // no-op today, but it keeps the set/clear pair symmetric for whichever
    // IST-guarded vector is the first to ever legitimately return (a
    // future NMI handler, most likely).
    if let Some((flag, _)) = guard {
        flag.store(false, Ordering::SeqCst);
    }
}
