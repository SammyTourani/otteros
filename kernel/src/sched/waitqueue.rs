//! Sleeping wait queues (brief M2-T1): the building block for the
//! sleeping `Mutex`/`Semaphore` (`sched::mutex`), `join`, and the
//! blocking keyboard reader (`drivers::ps2::keyboard`).
//!
//! Waking a thread only ever moves it from this queue's own waiters list
//! to the scheduler's ready queue -- brief M2-T1's design caution that
//! "wake-ups from IRQ context only enqueue" -- so `wake_one`/`wake_all`
//! are safe to call from IRQ context (the keyboard IRQ handler does
//! exactly that). Blocking (`wait_until`) is normal-thread-context only:
//! it is this thread parking *itself*, which an interrupt handler can
//! never do on another thread's behalf.

use alloc::collections::VecDeque;
use alloc::sync::Arc;

use super::thread::{Thread, ThreadState};
use super::{block_current, wake};
use crate::sync::IrqMutex;

pub struct WaitQueue {
    waiters: IrqMutex<VecDeque<Arc<Thread>>>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self { waiters: IrqMutex::new(VecDeque::new()) }
    }

    /// Blocks the current thread until `cond` returns `true`.
    ///
    /// Lost-wakeup-safe (brief M2-T1: "re-checks under the lock to avoid
    /// lost wake-ups"): `cond` is checked once up front (the fast,
    /// uncontended path), then, if that failed, checked *again* from
    /// inside `block_current`'s callback -- which runs with interrupts
    /// already disabled, the same interrupts-disabled window this thread
    /// registers itself as a waiter in. Since nothing else can run on
    /// this single core while interrupts are off, no waker anywhere can
    /// observe "not yet a waiter" and later find "already blocked and
    /// never told" -- the two are atomic with respect to each other. A
    /// wake that arrives after this thread is registered simply moves it
    /// back to the ready queue as usual; a wake that arrives before is
    /// caught by this second check instead, and this call returns
    /// without ever blocking.
    ///
    /// Also tolerates spurious wake-ups: after being woken, this loops
    /// back and re-checks `cond` from the top rather than assuming a
    /// single wake-up means the condition is now true.
    pub fn wait_until(&self, mut cond: impl FnMut() -> bool) {
        loop {
            if cond() {
                return;
            }
            let mut satisfied = false;
            block_current(|thread| {
                if cond() {
                    satisfied = true;
                    false // condition became true while registering: don't block after all.
                } else {
                    self.waiters.lock().push_back(thread);
                    true
                }
            });
            if satisfied {
                return;
            }
        }
    }

    /// Pops the longest-waiting *live* thread without waking it, skipping
    /// (and permanently discarding) any `Exited` entries in front of it,
    /// or `None` if nothing live is waiting. For a caller
    /// (`sched::mutex::Mutex::unlock`, `Semaphore::release`) that needs to
    /// hand something to a *specific* waiter before that thread can
    /// possibly run again -- `wake_one` itself is simpler and sufficient
    /// for anything that doesn't need that.
    ///
    /// Skipping `Exited` entries (kernel-review round 3) is what keeps
    /// `Mutex`/`Semaphore` from wedging forever: a thread `sched::
    /// force_exit` kills while parked here would otherwise still be the
    /// one `unlock`/`release` hands off to -- `wake` already refuses to
    /// actually resume it, but by then `unlock`/`release` have already
    /// taken the "someone is waiting" branch instead of the "really free
    /// it" one, and nothing ever calls either again for a thread that can
    /// never run. Discarding it here instead means `unlock`/`release` see
    /// a genuine `None` (no *live* waiter) and correctly fall through to
    /// releasing the resource for real.
    pub(crate) fn pop(&self) -> Option<Arc<Thread>> {
        let mut waiters = self.waiters.lock();
        while let Some(thread) = waiters.pop_front() {
            if thread.state() != ThreadState::Exited {
                return Some(thread);
            }
        }
        None
    }

    /// Wakes the longest-waiting thread on this queue, if any. Safe to
    /// call from IRQ context (see module docs).
    pub fn wake_one(&self) {
        let popped = self.pop();
        if let Some(thread) = popped {
            wake(thread);
        }
    }

    /// Wakes every thread currently waiting. Safe to call from IRQ
    /// context (see module docs).
    pub fn wake_all(&self) {
        loop {
            let popped = self.pop();
            match popped {
                Some(thread) => wake(thread),
                None => break,
            }
        }
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
