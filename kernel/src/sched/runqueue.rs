//! The scheduler's ready queue (brief M2-T1): FIFO `Arc<Thread>` handles,
//! capacity reserved once up front for every thread this kernel can ever
//! run concurrently (`super::MAX_THREADS`) so `push_back` never
//! allocates. That matters because it runs from inside the timer IRQ
//! path (`sched::schedule`, called from `preempt_if_needed` after EOI)
//! and from wake-up paths the keyboard IRQ handler reaches too -- both of
//! which must never allocate (brief M2-T1's design cautions).

use alloc::collections::VecDeque;
use alloc::sync::Arc;

use super::thread::Thread;

pub(crate) struct RunQueue {
    ready: VecDeque<Arc<Thread>>,
}

impl RunQueue {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self { ready: VecDeque::with_capacity(capacity) }
    }

    /// Adds `thread` to the back of the queue.
    ///
    /// # Panics
    /// If the queue is already at its reserved capacity. Every live
    /// thread is bounded by `super::MAX_THREADS` (enforced at `spawn`),
    /// and a thread is in this queue only while `Ready`-but-not-running,
    /// so this can only fire on a genuine scheduler bug, never a normal
    /// workload -- silently reallocating instead would risk exactly the
    /// IRQ-context allocation this type exists to rule out.
    pub(crate) fn push_back(&mut self, thread: Arc<Thread>) {
        assert!(
            self.ready.len() < self.ready.capacity(),
            "sched::RunQueue: exceeded its reserved capacity ({} threads)",
            self.ready.capacity()
        );
        self.ready.push_back(thread);
    }

    pub(crate) fn pop_front(&mut self) -> Option<Arc<Thread>> {
        self.ready.pop_front()
    }

    pub(crate) fn len(&self) -> usize {
        self.ready.len()
    }
}
