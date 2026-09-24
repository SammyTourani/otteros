//! A minimal spinlock (D2/CLAUDE.md: no OS-logic crates -- `spin` is on the
//! kernel's own allowlist, not this crate's, so the handful of lines this
//! needs are written out directly). OtterOS is single-threaded per process
//! today (DECISIONS.md D17), so contention never actually happens; this
//! exists so `alloc_impl`'s global state has real interior mutability
//! (rather than a `static mut`, which the 2024 edition's `static_mut_refs`
//! lint forbids taking a reference to) and stays correct the day a second
//! thread joins the same process.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: every access to `value` goes through `lock()`, which only ever
// hands out the guard after winning `locked`'s compare-exchange below --
// exactly the mutual-exclusion property a `Sync` bound requires from a
// type whose only unsynchronised field is an `UnsafeCell`.
unsafe impl<T: Send> Sync for SpinLock<T> {}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self { locked: AtomicBool::new(false), value: UnsafeCell::new(value) }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        SpinLockGuard { lock: self }
    }
}

impl<T> core::ops::Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: holding a `SpinLockGuard` is proof this lock is held
        // exclusively by the current thread (constructed only by `lock`
        // above, after winning the compare-exchange).
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> core::ops::DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: see `Deref::deref`.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}
