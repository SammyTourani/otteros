//! Sleeping synchronisation built on `WaitQueue` (brief M2-T1): a
//! non-recursive `Mutex<T>` and a counting `Semaphore`. These park the
//! calling thread instead of spinning, so they're for ordinary
//! thread-to-thread exclusion/signalling -- never for state an interrupt
//! handler also touches (that's still `sync::IrqMutex`'s job).

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::waitqueue::WaitQueue;
use super::{ThreadId, current, current_id, wake};

/// Sentinel for "no thread currently holds this mutex" -- real thread ids
/// start at 0 and count up, so this must be a value `sched` will never
/// actually assign; `u64::MAX` is safe for any workload this kernel could
/// plausibly run.
const NO_OWNER: ThreadId = u64::MAX;

/// A sleeping, non-recursive mutex.
pub struct Mutex<T> {
    locked: AtomicBool,
    owner: AtomicU64,
    queue: WaitQueue,
    data: UnsafeCell<T>,
}

// SAFETY: `Mutex<T>` only ever exposes `&mut T` (via `MutexGuard`) to
// whichever single thread currently holds `locked` -- exactly what
// `Send`/`Sync` require of a mutex around a `T: Send`. `T` need not be
// `Sync` itself: nothing ever hands out two live references to the data
// at once.
unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            owner: AtomicU64::new(NO_OWNER),
            queue: WaitQueue::new(),
            data: UnsafeCell::new(value),
        }
    }

    /// Blocks until this mutex is acquired.
    ///
    /// # Panics
    /// If the calling thread already holds this exact mutex (non-
    /// recursive, brief M2-T1: "panics on re-entry by the same thread"):
    /// blocking here would deadlock this thread against itself forever,
    /// which is strictly worse than panicking immediately.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let me = current_id();
        assert_ne!(
            self.owner.load(Ordering::Relaxed),
            me,
            "sched::Mutex::lock: thread {me} tried to re-lock a mutex it already holds"
        );

        self.queue.wait_until(|| {
            // A direct hand-off from `unlock` (kernel-review, M2-T1)
            // always wins over trying `compare_exchange` again: `locked`
            // never actually goes back to `false` when there's a waiter
            // to hand off to (see `unlock`), so a *different*,
            // not-yet-blocked caller can never barge in and steal it
            // between the hand-off and this thread actually running.
            current().take_handoff() || self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok()
        });
        self.owner.store(me, Ordering::Relaxed);
        MutexGuard { mutex: self }
    }

    /// Releases the mutex: if another thread is already waiting for it,
    /// ownership transfers directly to the longest-waiting one (`locked`
    /// stays `true` the whole time -- kernel-review, M2-T1's "direct
    /// hand-off" -- so a concurrent, not-yet-blocked `lock()` call can
    /// never win a `compare_exchange` race against a waiter that was
    /// already parked). Only actually frees the mutex (`locked = false`)
    /// when no one is waiting.
    fn unlock(&self) {
        self.owner.store(NO_OWNER, Ordering::Relaxed);
        match self.queue.pop() {
            Some(next) => {
                next.grant_handoff();
                wake(next);
            }
            None => self.locked.store(false, Ordering::Release),
        }
    }
}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: `lock()` only ever returns once this call won the
        // `compare_exchange` race, and `unlock` (in `Drop` below) is the
        // only way `locked` goes back to `false` -- so exactly one
        // `MutexGuard` for this `Mutex` exists at a time, making this
        // shared borrow exclusive in practice.
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: see `deref`; `&mut self` here additionally rules out
        // another live borrow *through this same guard* at the same time.
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.unlock();
    }
}

/// A counting semaphore: `acquire` blocks while no permits remain,
/// `release` returns one and wakes a waiter.
pub struct Semaphore {
    permits: AtomicU64,
    queue: WaitQueue,
}

impl Semaphore {
    pub const fn new(initial: u64) -> Self {
        Self { permits: AtomicU64::new(initial), queue: WaitQueue::new() }
    }

    pub fn acquire(&self) {
        self.queue.wait_until(|| {
            // See `Mutex::lock`'s identical reasoning: a direct hand-off
            // from `release` always wins, and `permits` is never
            // incremented for a hand-off (see `release`), so a
            // not-yet-blocked `acquire` can't steal the same permit via
            // `try_update` in between.
            current().take_handoff() || self.permits.try_update(Ordering::Acquire, Ordering::Relaxed, |p| p.checked_sub(1)).is_ok()
        });
    }

    /// Returns one permit: if another thread is already waiting for one,
    /// it transfers directly to the longest-waiting one (kernel-review,
    /// M2-T1's "direct hand-off" -- `permits` itself is left unchanged,
    /// so a concurrent, not-yet-blocked `acquire` can never grab the same
    /// permit first). Only actually increments `permits` when no one is
    /// waiting.
    pub fn release(&self) {
        match self.queue.pop() {
            Some(next) => {
                next.grant_handoff();
                wake(next);
            }
            None => {
                self.permits.fetch_add(1, Ordering::Release);
            }
        }
    }

    /// Current permit count (test/introspection only).
    pub fn available(&self) -> u64 {
        self.permits.load(Ordering::Relaxed)
    }
}
