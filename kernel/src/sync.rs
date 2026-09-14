//! `IrqMutex<T>` (brief M1-T5): a `spin::Mutex` whose guard disables
//! interrupts before taking the lock and restores RFLAGS.IF -- to
//! whatever it was immediately before *this* `lock()` call, not just an
//! unconditional `sti` -- when it drops. This is the one lock type
//! `serial::SERIAL1`, `mm::pmm` and `mm::heap` all use: now that
//! `arch::x86_64::irq::enable` turns interrupts on for good, a timer (or
//! any other) interrupt landing on this core while one of those locks is
//! already held would otherwise try to take it again from inside the
//! handler (`kprintln!`, an allocation, ...) and spin forever against
//! itself -- `spin::Mutex` isn't reentrant, and there's only one core
//! here to ever make progress on it again. Disabling interrupts for the
//! critical section's duration rules that out: nothing can preempt this
//! core while the lock is held, so it is always free again by the time
//! anything else on this core could want it.
//!
//! This mirrors `arch::x86_64::interrupts::without_interrupts` (which
//! predates interrupts ever being enabled at all, brief M1-T1) at the
//! granularity of a lock guard instead of a closure, so it can wrap a
//! `static` directly instead of requiring every access to go through a
//! closure-taking wrapper function.

use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use spin::{Mutex, MutexGuard};

use crate::arch::x86_64::interrupts::{cli, interrupts_enabled, sti};

/// An IRQ-safe spinlock: the same shape as `spin::Mutex`, except `lock()`
/// also disables interrupts first, and the returned guard restores
/// RFLAGS.IF (to whatever it was right before that particular `lock()`
/// call) when it drops. A nested `lock()` -- this same lock taken
/// recursively (which deadlocks exactly like `spin::Mutex` always would;
/// this type adds IRQ safety, not reentrancy), or a *different*
/// `IrqMutex` taken while this one is already held -- can therefore never
/// re-enable interrupts underneath an outer critical section that needs
/// them to stay off: only the guard that actually found interrupts
/// enabled (and so was the one that disabled them) ever turns them back
/// on.
pub struct IrqMutex<T: ?Sized> {
    inner: Mutex<T>,
}

impl<T> IrqMutex<T> {
    /// Wraps `value` in a new, unlocked `IrqMutex`.
    pub const fn new(value: T) -> Self {
        Self { inner: Mutex::new(value) }
    }
}

impl<T: ?Sized> IrqMutex<T> {
    /// Disables interrupts, then blocks until the lock is free. The
    /// returned guard restores RFLAGS.IF (to what it was right before
    /// this call) when it drops.
    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        let was_enabled = interrupts_enabled();
        // SAFETY: disabling interrupts is always valid from ring 0; the
        // matching restore happens in `IrqMutexGuard::drop` below,
        // exactly once, for exactly this call.
        unsafe { cli() };
        IrqMutexGuard { guard: ManuallyDrop::new(self.inner.lock()), was_enabled }
    }
}

/// RAII guard for `IrqMutex::lock`. Dropping it releases the inner lock
/// first, then -- only if this guard is the one that found interrupts
/// enabled on entry, and therefore the one that disabled them -- restores
/// them.
pub struct IrqMutexGuard<'a, T: ?Sized + 'a> {
    guard: ManuallyDrop<MutexGuard<'a, T>>,
    was_enabled: bool,
}

impl<T: ?Sized> Deref for IrqMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> DerefMut for IrqMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T: ?Sized> Drop for IrqMutexGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: every `IrqMutexGuard` is built (in `IrqMutex::lock`
        // above) with an initialised `guard`, and this `Drop` impl is the
        // only place that ever runs `ManuallyDrop::drop` on it -- exactly
        // once, since a value can only be dropped once.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        if self.was_enabled {
            // SAFETY: this only runs when this guard is the one that
            // found interrupts enabled (and therefore the one that
            // disabled them, in `IrqMutex::lock`), so it can't re-enable
            // interrupts underneath an outer `IrqMutex` guard (or
            // `arch::x86_64::interrupts::without_interrupts` call) that
            // found them already disabled.
            unsafe { sti() };
        }
    }
}
