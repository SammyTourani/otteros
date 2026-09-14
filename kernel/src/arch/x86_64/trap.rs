//! `TrapFrame` and the shared exception/interrupt dispatcher every stub in
//! `interrupts.rs` calls through `common_stub`.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use spin::Mutex;

use crate::kprintln_emergency;

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

static LAST_BREAKPOINT_REGS: Mutex<BreakpointRegs> = Mutex::new(BreakpointRegs {
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

/// Reads CR2, the faulting linear address the CPU latches on a page fault.
fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: reading CR2 has no side effects and is valid from ring 0 at
    // any time; this is a single register read with no memory access.
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
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
/// calls: `frame` points at a `TrapFrame` built on that stub's own stack
/// (either the interrupted stack, or the IST1/IST2 stack for vectors 8, 2
/// and 18; see gdt.rs).
///
/// # Safety
/// `frame` must point at a live, fully-initialized `TrapFrame`; only
/// `interrupts::common_stub` ever calls this, immediately after building
/// exactly that.
pub unsafe extern "C" fn trap_dispatch(frame: *mut TrapFrame) {
    // SAFETY: `common_stub` just built this `TrapFrame` in place on top of
    // its own stack and passed us that exact address in `rdi`; it's valid
    // for reads for the lifetime of this call, and nothing else can alias
    // it (interrupts are off and we're single-threaded until later tasks).
    let frame = unsafe { &*frame };

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
            let cr2 = read_cr2();
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
            kprintln_emergency!("EXCEPTION: DOUBLE FAULT");
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
        v => {
            kprintln_emergency!("[trap] unexpected vector {v}");
        }
    }

    // Normal-return path (breakpoint, or an as-yet-unused vector >= 32):
    // clear the guard flag, if this vector had one. Neither vector 3 nor
    // vector >= 32 ever sets one (see `ist_guard_for`), so this is a no-op
    // today, but it keeps the set/clear pair symmetric for whichever
    // IST-guarded vector is the first to ever legitimately return (a
    // future NMI handler, most likely).
    if let Some((flag, _)) = guard {
        flag.store(false, Ordering::SeqCst);
    }
}
