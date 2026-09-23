//! The idle thread (brief M2-T1): the scheduler's fallback when the ready
//! queue is empty. `sched::schedule` never puts `idle` in the ready
//! queue itself (it's picked by identity, not by sitting in line), and
//! nothing should ever count it as real work.

/// `idle`'s entry point: `hlt`s forever. Every `hlt` returns as soon as
/// any interrupt fires -- the periodic timer alone already guarantees
/// this loop keeps spinning through it, ready to be preempted the next
/// time something else is runnable.
pub(crate) fn idle_main(_arg: usize) -> i32 {
    loop {
        // SAFETY: `hlt` halts until the next interrupt and is always
        // valid to execute from ring 0; `idle` only ever runs once
        // interrupts are enabled for good (`sched::context::trampoline`
        // enables them before calling this), so it always wakes again.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)) };
    }
}
