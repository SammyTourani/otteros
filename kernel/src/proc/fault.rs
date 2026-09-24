//! Ring-3 fault handling (brief M2-T2 step 8): hooked from `arch::x86_64::
//! trap::trap_dispatch` whenever `frame.cs & 3 == 3`. A page fault inside
//! the faulting process's reserved-but-not-yet-mapped stack region demand-
//! grows it and resumes; every other page fault, and every ring-3 `#DE`/
//! `#UD`/`#SS`/`#GP`/`#AC`, kills only the offending process -- the kernel
//! itself must keep running either way (D14-style: this logs through the
//! lock-free emergency writer, the same as every other trap-path log, for
//! the same "a fault could interrupt code that already holds the normal
//! console/serial lock" reason).

use crate::arch::x86_64::trap::{self, TrapFrame};
use crate::kprintln_emergency;
use crate::mm::addr::VirtAddr;

/// Exit code every fault-killed process gets: the traditional shell
/// "128 + signal number" shape (brief M2-T2 step 8), using `11` (`SIGSEGV`)
/// uniformly rather than a different constant per exception -- the brief
/// asks for *a* 128+signal-like constant, not a precise signal mapping.
const FAULT_EXIT_CODE: i32 = 128 + 11;

/// Logs `[proc] pid <n> (<name>) killed: <exception> at rip=0x.. addr=0x..`
/// and ends the calling (faulting) thread's own process. Never returns.
fn kill_current_process(frame: &TrapFrame, exception: &str, addr: u64) -> ! {
    let process = super::current();
    kprintln_emergency!(
        "[proc] pid {} ({}) killed: {} at rip=0x{:x} addr=0x{:x}",
        process.pid(),
        process.name(),
        exception,
        frame.rip,
        addr
    );
    // Kernel-review: "make memory use flat" -- see `proc::exit_current_
    // process`'s identical fix; `exit_current_process` below never
    // returns here, so `process` must be dropped explicitly first.
    drop(process);
    super::exit_current_process(FAULT_EXIT_CODE);
}

/// `trap_dispatch`'s vector-14 (`#PF`) hook for a fault taken from ring 3.
/// Resolves it (and returns, letting `common_stub`'s `iretq` simply retry
/// the faulting instruction) if `cr2` falls inside the calling process's
/// reserved stack region and demand-growing it succeeds; kills the process
/// otherwise.
pub(crate) fn handle_page_fault(frame: &TrapFrame, cr2: u64) {
    let process = super::current();
    if process.try_grow_stack(VirtAddr::new(cr2)) {
        return;
    }
    // Kernel-review: "make memory use flat" -- `kill_current_process`
    // below never returns, so `process` must be dropped explicitly first
    // (see that function's own identical fix).
    drop(process);
    kill_current_process(frame, "PAGE FAULT", cr2);
}

/// `trap_dispatch`'s hook for `#DE`/`#UD`/`#SS`/`#GP`/`#AC` taken from ring
/// 3 (brief M2-T2 step 8) -- none of these are ever resolvable the way a
/// stack-growth page fault is, so this always kills the process. `addr` is
/// the exception's own error code (`#GP`/`#SS` carry a segment-selector-
/// related index there; `#DE`/`#UD`/`#AC` always push `0`) -- there is no
/// CR2-like faulting address for any of these, so it's the closest
/// available diagnostic to report in that slot.
pub(crate) fn handle_other_fault(frame: &TrapFrame) {
    kill_current_process(frame, trap::mnemonic(frame.vector), frame.error_code);
}
