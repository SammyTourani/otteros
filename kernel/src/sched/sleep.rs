//! Timed sleeping (brief M2-T1): `sleep_ms` blocks the calling thread
//! until a target tick count; `wake_due`, called from the timer tick
//! (IRQ context, via `sched::on_timer_tick`), moves every thread whose
//! target has passed onto the ready queue and nothing else -- brief
//! M2-T1's design caution that IRQ-context wake-ups "touch scheduler
//! state only under the lock, never allocate, never log" holds here:
//! `wake_due` only scans and moves entries already reserved room for, it
//! never grows anything.

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::runqueue::RunQueue;
use super::thread::{Thread, ThreadState};
use super::{block_current, with_sched};
use crate::time;

pub(crate) struct SleepEntry {
    wake_at: u64,
    pub(crate) thread: Arc<Thread>,
}

/// Blocks the current thread until at least `ms` milliseconds (rounded to
/// the nearest tick, brief M2-T1: 1 kHz nominal) have passed.
pub fn sleep_ms(ms: u64) {
    let wake_at = time::ticks().saturating_add(ms);
    block_current(|thread| {
        thread.set_state(ThreadState::Sleeping);
        with_sched(|s| s.sleeping.push(SleepEntry { wake_at, thread: thread.clone() }));
        true // always actually block: there is no condition to double-check here.
    });
}

/// Moves every entry whose `wake_at` has passed from `sleeping` onto
/// `ready`, returning whether it moved at least one (kernel-review,
/// M2-T1: the caller, `on_timer_tick`, uses this to decide whether to
/// request an immediate reschedule -- see its own docs). In-place
/// removal only (`Vec::swap_remove`) -- never allocates, safe to call
/// with `sleeping`/`ready` already locked from IRQ context.
pub(crate) fn wake_due(sleeping: &mut Vec<SleepEntry>, ready: &mut RunQueue, now: u64) -> bool {
    let mut woke_any = false;
    let mut i = 0;
    while i < sleeping.len() {
        if sleeping[i].wake_at <= now {
            let entry = sleeping.swap_remove(i);
            // Brief M2-T2: `force_exit` (`proc::kill`) may have marked
            // this thread `Exited` while it was still sleeping -- unlike
            // every ordinary sleeper, whose state is still exactly
            // `Sleeping` here, that must never be resurrected back to
            // `Ready` (mirrors `wake`'s own `matches!(Blocked | Sleeping)`
            // guard for a `WaitQueue`-parked thread).
            if entry.thread.state() == ThreadState::Sleeping {
                entry.thread.set_state(ThreadState::Ready);
                ready.push_back(entry.thread);
                woke_any = true;
            }
        } else {
            i += 1;
        }
    }
    woke_any
}
