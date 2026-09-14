//! `sync::IrqMutex` and `arch::x86_64::irq` dispatch tests (brief M1-T5):
//! nested guards restore RFLAGS.IF correctly, a registered handler fires
//! when its vector is raised with `int <vector>`, and an unregistered
//! vector is counted as spurious without disturbing execution.

use core::sync::atomic::{AtomicUsize, Ordering};

use otteros_kernel::arch::x86_64::interrupts;
use otteros_kernel::arch::x86_64::irq;
use otteros_kernel::arch::x86_64::trap::TrapFrame;
use otteros_kernel::sync::IrqMutex;

/// Two `IrqMutex`es, one taken while the other is already held: dropping
/// the inner guard must not re-enable interrupts while the outer one is
/// still held, and only dropping the outermost guard should actually
/// restore RFLAGS.IF.
#[test_case]
fn irq_mutex_nested_guards_restore_if() {
    static A: IrqMutex<u32> = IrqMutex::new(1);
    static B: IrqMutex<u32> = IrqMutex::new(2);

    assert!(interrupts::interrupts_enabled(), "test should start with interrupts on");

    {
        let _a = A.lock();
        assert!(!interrupts::interrupts_enabled(), "taking the outer lock should have cleared IF");
        {
            let _b = B.lock();
            assert!(!interrupts::interrupts_enabled(), "the inner lock shouldn't re-enable IF while still held");
        }
        assert!(
            !interrupts::interrupts_enabled(),
            "dropping the inner guard shouldn't re-enable IF while the outer one is still held"
        );
    }
    assert!(interrupts::interrupts_enabled(), "dropping the outer guard should restore IF");
}

/// Vector used by `irq_registered_handler_fires_on_int`. Chosen well
/// above every vector this task's own drivers register (32 for the
/// timer, 0xFF for the LAPIC spurious vector).
const TEST_HANDLER_VECTOR: u8 = 200;
/// Vector used by `irq_unregistered_vector_counts_spurious_and_continues`
/// -- distinct from `TEST_HANDLER_VECTOR` so the two tests can't
/// interfere with each other regardless of run order.
const TEST_UNREGISTERED_VECTOR: u8 = 201;

static HANDLER_FIRED: AtomicUsize = AtomicUsize::new(0);

fn counting_handler(_frame: &mut TrapFrame) {
    HANDLER_FIRED.fetch_add(1, Ordering::SeqCst);
}

/// Registers a handler for `TEST_HANDLER_VECTOR`, raises it with a
/// software `int`, and confirms the handler ran exactly once and
/// execution resumed right here afterward -- the same "software
/// interrupt returns control" pattern `test_cases::traps::
/// breakpoint_returns` already relies on for `int3`, extended to an
/// arbitrary IRQ vector through `irq::dispatch`.
#[test_case]
fn irq_registered_handler_fires_on_int() {
    let before = HANDLER_FIRED.load(Ordering::SeqCst);
    irq::register(TEST_HANDLER_VECTOR, counting_handler);

    // SAFETY: `TEST_HANDLER_VECTOR` (200) is >= 32 (an IRQ, not a CPU
    // exception), has a present, DPL0 IDT gate (every vector does,
    // `idt::init`), and this core's CPL is 0, so the software `int` is
    // allowed; the handler just registered above only touches a local
    // atomic and returns normally, exactly like `int3` already does for
    // the breakpoint tests.
    unsafe {
        core::arch::asm!("int {vec}", vec = const TEST_HANDLER_VECTOR, options(nomem, nostack, preserves_flags));
    }

    assert_eq!(HANDLER_FIRED.load(Ordering::SeqCst), before + 1, "the registered handler should have fired exactly once");
    irq::unregister(TEST_HANDLER_VECTOR);
}

/// Raises a vector nothing has ever registered a handler for: `irq::
/// dispatch`'s fallback must count it spurious (not panic, not hang) and
/// execution must resume right here afterward.
#[test_case]
fn irq_unregistered_vector_counts_spurious_and_continues() {
    let before = irq::spurious_count();

    // SAFETY: see `irq_registered_handler_fires_on_int`; `trap_dispatch`'s
    // fallback for a vector with no registered handler counts it and
    // returns.
    unsafe {
        core::arch::asm!("int {vec}", vec = const TEST_UNREGISTERED_VECTOR, options(nomem, nostack, preserves_flags));
    }

    assert_eq!(irq::spurious_count(), before + 1, "an unregistered vector should be counted as spurious");
}
