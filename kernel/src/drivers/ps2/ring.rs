//! A lock-free single-producer/single-consumer ring buffer (brief M1-T6):
//! `drivers::ps2`'s IRQ1 handler is the single producer (`push`, called
//! only from that interrupt context, which must never block or take a
//! lock another context might already hold), `keyboard::poll_event` is
//! the single consumer (`pop`, called only from normal, non-interrupt
//! context). Two independent atomic cursors, no lock: this is exactly the
//! one-producer-one-consumer case where that's sound (see `push`/`pop`'s
//! own docs for why the two can never race on the same slot).
//!
//! Capacity is `N - 1` usable slots, not `N`: the classic trick that lets
//! "empty" (`head == tail`) and "full" (`head + 1 == tail`) stay
//! distinguishable with only two plain indices and no separate counter.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub struct SpscRing<T, const N: usize> {
    buf: [UnsafeCell<MaybeUninit<T>>; N],
    /// Next slot the producer will write. Only ever written by `push`.
    head: AtomicUsize,
    /// Next slot the consumer will read. Only ever written by `pop`.
    tail: AtomicUsize,
    /// Bytes `push` discarded because the ring was full.
    dropped: AtomicU64,
}

// SAFETY: `SpscRing` hands values of `T` from whichever context calls
// `push` to whichever calls `pop`; that is exactly what `Send` means for
// `T`, and the ring's own fields (two atomics and a drop counter) are
// already safe to share. The `UnsafeCell<MaybeUninit<T>>` slots are never
// aliased mutably across the two sides at once -- see `push`/`pop`.
unsafe impl<T: Send, const N: usize> Sync for SpscRing<T, N> {}

impl<T: Copy, const N: usize> SpscRing<T, N> {
    /// An empty ring. `const fn` so it can back a `static` directly (like
    /// `arch::x86_64::irq::HANDLERS`).
    pub const fn new() -> Self {
        Self {
            buf: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Pushes `value`. If the ring is already full, `value` (the newest
    /// byte) is dropped and counted (`dropped`) rather than overwriting a
    /// slot the consumer hasn't read yet, or blocking -- the only choice
    /// available to a call that must never block (brief M1-T6: "overflow
    /// drops the newest byte and counts it").
    pub fn push(&self, value: T) {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let next = (head + 1) % N;
        if next == tail {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        // SAFETY: `head` is only ever produced by this single producer
        // (`push`'s own prior calls); `next != tail` (just checked) means
        // slot `head` is not the one the consumer will read next, i.e. it
        // holds no value the consumer might still be reading -- safe to
        // overwrite. The `Release` store below publishes this write: a
        // consumer's matching `Acquire` load of `head` (`pop`) is
        // guaranteed to see it.
        unsafe { (*self.buf[head].get()).write(value) };
        self.head.store(next, Ordering::Release);
    }

    /// Pops the oldest pushed value not yet read, or `None` if the ring
    /// is empty.
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        // SAFETY: `tail != head`, so slot `tail` holds a value `push`
        // wrote before its matching `Release` store of `head` -- the
        // `Acquire` load just above synchronizes with that store, so the
        // write is visible here. Only this single consumer ever reads or
        // advances `tail`, so no other call can be reading this same slot
        // concurrently. `T: Copy` makes `MaybeUninit<T>` itself `Copy`,
        // so this reads a copy of the slot rather than moving out of
        // memory `push` might reuse before this returns.
        let value = unsafe { (*self.buf[tail].get()).assume_init() };
        self.tail.store((tail + 1) % N, Ordering::Release);
        Some(value)
    }

    /// How many pushed values have been dropped because the ring was
    /// full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Whether the ring currently has nothing left to `pop` (brief
    /// M2-T1: the condition `keyboard::read_char_blocking` waits on).
    pub fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Relaxed) == self.head.load(Ordering::Acquire)
    }
}

// `new()` is `const fn` (so it can back a `static` directly) and so can't
// itself be the `Default::default()` clippy's `new_without_default` wants
// -- `Default::default` isn't `const`-callable -- but nothing stops this
// separate, ordinary impl from just forwarding to it.
impl<T: Copy, const N: usize> Default for SpscRing<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
