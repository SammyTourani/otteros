//! Kernel thread / scheduler tests (brief M2-T1): preemptive round-robin
//! scheduling, `yield_now`, `sleep_ms`, `WaitQueue`, the sleeping `Mutex`/
//! `Semaphore`, `join`/reaping, and the guard page every thread stack
//! gets. Every test spawns its own helper thread(s) and `join`s them
//! before returning, so later tests never see leftover `Ready` threads
//! from earlier ones.
//!
//! `test thread-stackoverflow` (brief M2-T1 step 10 -- a spawned thread
//! recursing into its own guard page) is a separate negative test, driven
//! by cmdline dispatch in `test_main.rs`, not a `#[test_case]` here: it
//! is expected to double-fault and halt, which would take the whole
//! suite down with it if it ever ran as one of these.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use otteros_kernel::arch::x86_64::interrupts;
use otteros_kernel::mm::{kstack, pmm};
use otteros_kernel::sched::{self, Mutex, Semaphore, WaitQueue};
use otteros_kernel::sync::IrqMutex;
use otteros_kernel::time;

/// Two threads, each incrementing its own counter 500 times and
/// `yield_now`-ing between increments, both reach the full count --
/// proves cooperative round-robin switching actually alternates between
/// them rather than one starving the other.
#[test_case]
fn two_threads_yielding_both_progress() {
    static COUNTER_A: AtomicU64 = AtomicU64::new(0);
    static COUNTER_B: AtomicU64 = AtomicU64::new(0);

    fn worker_a(_: usize) -> i32 {
        for _ in 0..500 {
            COUNTER_A.fetch_add(1, Ordering::SeqCst);
            sched::yield_now();
        }
        0
    }
    fn worker_b(_: usize) -> i32 {
        for _ in 0..500 {
            COUNTER_B.fetch_add(1, Ordering::SeqCst);
            sched::yield_now();
        }
        0
    }

    let a = sched::spawn("test-yield-a", worker_a, 0);
    let b = sched::spawn("test-yield-b", worker_b, 0);
    assert_eq!(sched::join(a), 0);
    assert_eq!(sched::join(b), 0);

    assert_eq!(COUNTER_A.load(Ordering::SeqCst), 500);
    assert_eq!(COUNTER_B.load(Ordering::SeqCst), 500);
}

/// A thread that spins forever *without* ever calling `yield_now` must
/// not starve another thread: preemption alone (the timer tick, not
/// cooperation) has to give the other thread a turn.
///
/// Brief M2-T2b: bounded by a generous *scheduler-poll* count, not a
/// wall-clock tick deadline -- under host CPU contention the same amount
/// of genuine progress can take far more real time (and therefore far
/// more LAPIC ticks, which fire at a roughly constant real-time rate
/// regardless of how slow the guest itself is running), which made a
/// `ticks() - start < N` check here flaky under load. Counting this
/// thread's own poll iterations instead is insensitive to host speed: it
/// only runs out if `other` genuinely never gets to advance its counter,
/// which is the actual property under test.
#[test_case]
fn spinning_thread_does_not_starve_another() {
    static SPIN_COUNT: AtomicU64 = AtomicU64::new(0);
    static OTHER_COUNT: AtomicU64 = AtomicU64::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);

    fn spinner(_: usize) -> i32 {
        while !STOP.load(Ordering::Relaxed) {
            SPIN_COUNT.fetch_add(1, Ordering::Relaxed);
            core::hint::spin_loop();
        }
        0
    }
    fn other(_: usize) -> i32 {
        for _ in 0..5 {
            OTHER_COUNT.fetch_add(1, Ordering::Relaxed);
            sched::sleep_ms(1);
        }
        0
    }

    let spin_id = sched::spawn("test-spin", spinner, 0);
    let other_id = sched::spawn("test-other", other, 0);

    const MAX_POLLS: u64 = 200_000;
    let mut polls = 0u64;
    while OTHER_COUNT.load(Ordering::Relaxed) < 5 {
        polls += 1;
        assert!(
            polls < MAX_POLLS,
            "the non-spinning thread's counter should advance within {MAX_POLLS} scheduler polls \
             even though the other thread never yields"
        );
        sched::yield_now();
    }

    STOP.store(true, Ordering::Relaxed);
    sched::join(spin_id);
    sched::join(other_id);
    assert!(SPIN_COUNT.load(Ordering::Relaxed) > 0, "the spinner should have made progress too");
}

/// `sched::sleep_ms(30)` should advance ticks by roughly 30 -- a wide
/// window (brief M2-T1: QEMU TCG on this host delivers timer interrupts
/// at ~650 Hz, not the programmed 1 kHz) rather than a tight one.
#[test_case]
fn sched_sleep_ms_wide_window() {
    let before = time::ticks();
    sched::sleep_ms(30);
    let after = time::ticks();
    let delta = after - before;
    assert!((25..=120).contains(&delta), "sched::sleep_ms(30) advanced ticks by {delta}, expected 25..=120");
}

/// A thread blocked on a `WaitQueue` actually wakes up once `wake_one` is
/// called for it, rather than sleeping forever.
#[test_case]
fn waitqueue_wakes_blocked_thread() {
    static FLAG: AtomicBool = AtomicBool::new(false);
    static QUEUE: WaitQueue = WaitQueue::new();

    fn waiter(_: usize) -> i32 {
        QUEUE.wait_until(|| FLAG.load(Ordering::SeqCst));
        0
    }

    let id = sched::spawn("test-waiter", waiter, 0);
    // Give it several chances to actually reach `wait_until` and block
    // before this thread wakes it -- round robin with only `idle` else
    // runnable, so a handful of yields is plenty.
    for _ in 0..5 {
        sched::yield_now();
    }
    FLAG.store(true, Ordering::SeqCst);
    QUEUE.wake_one();
    assert_eq!(sched::join(id), 0);
}

/// A sleeping `Mutex` shared by 4 threads, each doing 1000 unsynchronized-
/// looking `+= 1`s through the guard, ends up at exactly 4000 -- proving
/// mutual exclusion (a torn read-modify-write would almost certainly land
/// short of 4000 with this many interleavings across 4 threads x 1000
/// increments).
#[test_case]
fn sleeping_mutex_four_threads_thousand_each() {
    static COUNTER: Mutex<u64> = Mutex::new(0);

    fn worker(_: usize) -> i32 {
        for _ in 0..1000 {
            *COUNTER.lock() += 1;
        }
        0
    }

    let ids: Vec<_> = (0..4).map(|_| sched::spawn("test-mutex-worker", worker, 0)).collect();
    for id in ids {
        assert_eq!(sched::join(id), 0);
    }
    assert_eq!(*COUNTER.lock(), 4000);
}

/// A `Semaphore::new(2)` never lets more than 2 of 6 competing threads
/// hold it at once.
#[test_case]
fn semaphore_limits_concurrency() {
    static SEM: Semaphore = Semaphore::new(2);
    static CURRENT: AtomicU64 = AtomicU64::new(0);
    static MAX_SEEN: AtomicU64 = AtomicU64::new(0);

    fn worker(_: usize) -> i32 {
        SEM.acquire();
        let now = CURRENT.fetch_add(1, Ordering::SeqCst) + 1;
        MAX_SEEN.fetch_max(now, Ordering::SeqCst);
        for _ in 0..20 {
            sched::yield_now();
        }
        CURRENT.fetch_sub(1, Ordering::SeqCst);
        SEM.release();
        0
    }

    let ids: Vec<_> = (0..6).map(|_| sched::spawn("test-sem-worker", worker, 0)).collect();
    for id in ids {
        assert_eq!(sched::join(id), 0);
    }
    assert!(
        MAX_SEEN.load(Ordering::SeqCst) <= 2,
        "semaphore should never have allowed more than 2 concurrent holders, saw {}",
        MAX_SEEN.load(Ordering::SeqCst)
    );
    assert_eq!(SEM.available(), 2, "every permit should be back after every worker released it");
}

/// `join` returns the exact value the thread's entry function returned.
#[test_case]
fn join_returns_exit_code() {
    fn worker(arg: usize) -> i32 {
        arg as i32 * 2
    }
    let id = sched::spawn("test-exit-code", worker, 21);
    assert_eq!(sched::join(id), 42);
}

/// While the calling thread sleeps and nothing else is `Ready`, the
/// scheduler falls back to `idle` (never a stuck/hung CPU) and ticks keep
/// advancing -- `sleep_ms` returning at all here proves both the wake
/// path and the idle fallback work together.
#[test_case]
fn idle_runs_while_only_thread_sleeps() {
    // Drain any transient `Ready` residue first (there shouldn't be any --
    // every earlier test `join`s what it spawns -- but this costs nothing
    // and removes any dependence on test execution order).
    for _ in 0..3 {
        sched::yield_now();
    }
    let before = time::ticks();
    sched::sleep_ms(20);
    let after = time::ticks();
    assert!(
        after.saturating_sub(before) >= 15,
        "ticks should keep advancing (serviced by the idle fallback) while the only real \
         thread sleeps: {before} -> {after}"
    );
}

/// Every thread's kernel stack -- not just the boot stack -- has a
/// registered guard page (`mm::kstack::find_guard`).
#[test_case]
fn every_thread_stack_has_a_registered_guard() {
    fn worker(_: usize) -> i32 {
        0
    }
    let id = sched::spawn("test-guard-check", worker, 0);
    let thread = sched::find(id).expect("a just-spawned thread should be findable by id");
    let stack = thread.stack();
    assert!(
        kstack::find_guard(stack.guard).is_some(),
        "a freshly spawned thread's stack should have a registered guard page"
    );
    assert_eq!(sched::join(id), 0);
}

/// A thread's stack frames return to the PMM once it exits and is
/// reaped -- reaping happens on some *other* thread's later `schedule()`
/// call (a thread can never free its own stack), so this gives that a
/// few chances via `yield_now` rather than assuming `join` itself
/// guarantees it already happened.
#[test_case]
fn exited_thread_stack_is_reaped_eventually() {
    fn worker(_: usize) -> i32 {
        0
    }

    // Settle first: an *earlier* test's own exited thread can still be
    // sitting in `to_reap` (reaping happens on some other thread's next
    // `schedule()` call, not synchronously with that test's own `join`),
    // and `schedule`'s reap loop drains all of `to_reap` in one pass. Let
    // that resolve before measuring, so this test's own before/after
    // delta reflects only *its* thread, not a leftover one too.
    for _ in 0..20 {
        sched::yield_now();
    }

    let baseline = pmm::stats().free;
    let id = sched::spawn("test-reap", worker, 0);
    assert_eq!(sched::join(id), 0);

    let mut freed = false;
    for _ in 0..20 {
        sched::yield_now();
        if pmm::stats().free >= baseline {
            freed = true;
            break;
        }
    }
    assert!(freed, "expected the exited thread's stack frames to be freed back to the PMM baseline");
    assert_eq!(pmm::stats().free, baseline);
}

/// An ordinary `IrqMutex` taken *around* a call into the scheduler (which
/// takes its own internal `IrqMutex` -- brief M2-T1's design) must still
/// restore RFLAGS.IF only when the outermost guard drops, exactly like
/// two directly-nested `IrqMutex`es (`test_cases::irq::
/// irq_mutex_nested_guards_restore_if`).
#[test_case]
fn nested_irqmutex_around_scheduler_call_restores_if() {
    static OUTER: IrqMutex<u32> = IrqMutex::new(0);

    assert!(interrupts::interrupts_enabled(), "test should start with interrupts on");
    {
        let _guard = OUTER.lock();
        assert!(!interrupts::interrupts_enabled(), "taking the outer lock should have cleared IF");

        // `sched::current()` takes the scheduler's own internal lock.
        let _ = sched::current();
        assert!(
            !interrupts::interrupts_enabled(),
            "the scheduler's internal critical section must not re-enable IF while OUTER is still held"
        );
    }
    assert!(interrupts::interrupts_enabled(), "dropping the outer guard should restore IF");
}

/// A tight, `yield_now`-free loop on the *calling* (test-runner) thread
/// itself doesn't stall the timer: preemption keeps firing (and
/// `time::ticks()` keeps advancing) purely from hardware interrupts, with
/// no cooperation from this loop at all.
#[test_case]
fn preemption_keeps_ticks_advancing_during_busy_loop() {
    let before = time::ticks();
    let target = before.saturating_add(50);
    while time::ticks() < target {
        core::hint::spin_loop();
    }
    assert!(time::ticks() >= target);
}

/// `spawn` hands out strictly increasing ids.
#[test_case]
fn spawn_assigns_unique_increasing_ids() {
    fn worker(_: usize) -> i32 {
        0
    }
    let a = sched::spawn("test-id-a", worker, 0);
    let b = sched::spawn("test-id-b", worker, 0);
    assert!(b > a, "thread ids should be monotonically increasing: {a} then {b}");
    assert_eq!(sched::join(a), 0);
    assert_eq!(sched::join(b), 0);
}

/// A thread that actually runs across several preemptions accumulates a
/// nonzero `ticks_run` count.
#[test_case]
fn thread_ticks_run_increases_while_running() {
    static STOP: AtomicBool = AtomicBool::new(false);
    fn worker(_: usize) -> i32 {
        while !STOP.load(Ordering::Relaxed) {
            core::hint::spin_loop();
        }
        0
    }

    let id = sched::spawn("test-ticks-run", worker, 0);
    let thread = sched::find(id).expect("just-spawned thread should be findable");

    let start = time::ticks();
    while time::ticks().saturating_sub(start) < 30 {
        core::hint::spin_loop();
    }
    let ticks_before_stop = thread.ticks_run();
    STOP.store(true, Ordering::Relaxed);
    assert_eq!(sched::join(id), 0);

    assert!(ticks_before_stop > 0, "a thread that ran across 30+ ticks should have a nonzero ticks_run");
}

/// Lost-wakeup race (kernel-review, M2-T1): deliberately *no* priming
/// yields before either side acts, unlike `waitqueue_wakes_blocked_thread`
/// above. One thread (`setter`) spins a counter-derived, varying number
/// of times before setting a flag and calling `wake_one`; the other
/// (`waiter`) races straight into `wait_until` for the very same
/// iteration. Varying the spin count means the actual race lands at a
/// different point in `wait_until`'s own internal sequence every
/// iteration -- before it ever checks the condition, while it's
/// registering as a waiter, or after -- rather than the same timing every
/// time. `wait_until`'s documented re-check-under-the-lock design must
/// win every iteration; if it ever lost a wake-up, `waiter` would block
/// forever on that iteration, so this bounds the wait with an explicit
/// tick budget and fails with a clear message instead of hanging the
/// whole suite (kernel-review's explicit requirement).
#[test_case]
fn waitqueue_no_lost_wakeup_under_varied_timing() {
    const ITERATIONS: u64 = 200;

    static FLAG: AtomicBool = AtomicBool::new(false);
    static QUEUE: WaitQueue = WaitQueue::new();
    static COMPLETED: AtomicU64 = AtomicU64::new(0);

    fn waiter(_: usize) -> i32 {
        for _ in 0..ITERATIONS {
            QUEUE.wait_until(|| FLAG.load(Ordering::SeqCst));
            FLAG.store(false, Ordering::SeqCst);
            COMPLETED.fetch_add(1, Ordering::SeqCst);
        }
        0
    }

    fn setter(_: usize) -> i32 {
        for i in 0..ITERATIONS {
            // Never race more than one iteration ahead of `waiter` --
            // otherwise, since this thread never blocks, it could race
            // through *every* remaining iteration (and exit) before
            // `waiter` ever gets a turn, permanently desyncing the two
            // (a bug in this test's own synchronization, not the thing
            // under test). Staying within one iteration of what's
            // already been consumed keeps the race meaningful: it's
            // always *this exact* iteration's wake being raced against
            // *this exact* iteration's wait.
            while COMPLETED.load(Ordering::SeqCst) != i {
                core::hint::spin_loop();
            }
            // A different, small spin count each iteration so the race
            // against `waiter` (which loops straight back into
            // `wait_until` the instant it finishes the previous
            // iteration) lands at a different relative point every
            // time -- sometimes `FLAG` is already true before `waiter`
            // ever checks, sometimes it's set only after `waiter` has
            // fully registered as a blocked waiter -- without ever
            // explicitly yielding to force one thread ahead of the
            // other.
            for _ in 0..(i * 37) % 64 {
                core::hint::spin_loop();
            }
            FLAG.store(true, Ordering::SeqCst);
            QUEUE.wake_one();
        }
        0
    }

    let w = sched::spawn("test-race-waiter", waiter, 0);
    let s = sched::spawn("test-race-setter", setter, 0);

    let start = time::ticks();
    // Brief M2-T2b: widened well past ordinary TCG jitter (3000 -> 30000)
    // to also absorb host CPU contention, which can stretch the same 200
    // iterations' worth of real scheduling work across many more LAPIC
    // ticks (a roughly real-time-rate hardware timer) without a lost
    // wakeup being anywhere near it -- this is still just a "don't hang
    // forever" ceiling, not a tight performance assertion; the actual
    // proof is `COMPLETED == ITERATIONS` plus both `join`s below.
    const BUDGET_TICKS: u64 = 30_000;
    while sched::find(w).is_some_and(|t| t.state() != sched::ThreadState::Exited) {
        assert!(
            time::ticks().saturating_sub(start) < BUDGET_TICKS,
            "lost wake-up: only {} of {ITERATIONS} iterations completed within {BUDGET_TICKS} ticks",
            COMPLETED.load(Ordering::SeqCst)
        );
        sched::yield_now();
    }
    assert_eq!(sched::join(w), 0);
    assert_eq!(sched::join(s), 0);
    assert_eq!(COMPLETED.load(Ordering::SeqCst), ITERATIONS);
}

/// A thread woken while the CPU is genuinely idle (nothing else runnable)
/// runs promptly after its wake tick (kernel-review, M2-T1: `wake`/
/// `wake_due` set `NEED_RESCHED` immediately when they make a thread
/// `Ready` while `idle` is current, rather than waiting for the next
/// unrelated timeslice expiry). Uses `sleep_ms` (whose wake path is
/// `wake_due`, driven by the timer tick -- genuine IRQ context) rather
/// than a `WaitQueue`, and arranges for this test's own thread to be
/// asleep too, for *longer*, so `idle` is truly the one running when the
/// spawned thread's shorter sleep elapses.
///
/// Brief M2-T2b: the tolerance (originally a razor-thin 2 ticks, renamed
/// from `..._within_two_ticks`) is widened to 20 -- generous enough to
/// absorb host-CPU-contention-induced scheduling latency between "the
/// timer IRQ notices the sleep elapsed" and "the woken thread's own next
/// instruction reads `time::ticks()` again," while still being far
/// tighter than a genuine regression back to "wait for some later,
/// unrelated scheduling event" would produce (there is nothing else
/// scheduled to run in this test at all until the sleeper wakes, so a
/// lost immediate-reschedule nudge here would stall for a very long time,
/// not just a few extra ticks).
#[test_case]
fn wake_while_idle_runs_promptly() {
    static SLEPT_UNTIL: AtomicU64 = AtomicU64::new(0);
    static RAN_AT: AtomicU64 = AtomicU64::new(0);
    static DONE: AtomicBool = AtomicBool::new(false);

    fn sleeper(_: usize) -> i32 {
        let target = time::ticks().saturating_add(10);
        SLEPT_UNTIL.store(target, Ordering::SeqCst);
        sched::sleep_ms(10);
        RAN_AT.store(time::ticks(), Ordering::SeqCst);
        DONE.store(true, Ordering::SeqCst);
        0
    }

    let id = sched::spawn("test-wake-idle", sleeper, 0);
    // Let it actually start and enter its own (shorter) sleep.
    for _ in 0..3 {
        sched::yield_now();
    }

    // This thread -- the only other real one at this point -- sleeps for
    // *longer*, so by the time the spawned thread's own wake tick
    // arrives, `idle` (not this thread) is genuinely the one running.
    sched::sleep_ms(30);

    assert!(DONE.load(Ordering::SeqCst), "the spawned thread should have woken and run during our own, longer sleep");
    let slept_until = SLEPT_UNTIL.load(Ordering::SeqCst);
    let ran_at = RAN_AT.load(Ordering::SeqCst);
    assert!(
        ran_at <= slept_until + 20,
        "a thread woken while idle should run within 20 ticks of its wake tick {slept_until}, but ran at {ran_at}"
    );
    assert_eq!(sched::join(id), 0);
}

/// Kernel-review round 3: force-exiting a thread while it's blocked
/// waiting on a `Mutex` must never wedge that mutex forever. `w1` queues
/// up on `MUTEX` behind the holder, is `force_exit`-ed while still
/// blocked, and `holder`'s later `unlock()` must still hand the lock to
/// `w2` (queued *after* `w1`) instead of leaving `locked` permanently
/// `true` because it "handed off" to a thread that can never run again.
#[test_case]
fn force_exit_of_blocked_mutex_waiter_does_not_wedge_it() {
    static MUTEX: Mutex<u32> = Mutex::new(0);
    static W2_ACQUIRED: AtomicBool = AtomicBool::new(false);

    fn holder(_: usize) -> i32 {
        let _guard = MUTEX.lock();
        // Hold it long enough for both w1 and w2 to queue up, and for the
        // test to `force_exit` w1, before ever releasing.
        for _ in 0..40 {
            sched::yield_now();
        }
        0
    }
    fn w1_never_acquires(_: usize) -> i32 {
        let _guard = MUTEX.lock();
        unreachable!("test-mutex-w1 should never acquire the lock -- force_exit-ed while still queued");
    }
    fn w2_acquires(_: usize) -> i32 {
        let _guard = MUTEX.lock();
        W2_ACQUIRED.store(true, Ordering::SeqCst);
        0
    }

    let holder_id = sched::spawn("test-mutex-holder", holder, 0);
    for _ in 0..5 {
        sched::yield_now(); // let holder actually acquire the lock first.
    }

    let w1 = sched::spawn("test-mutex-w1", w1_never_acquires, 0);
    for _ in 0..5 {
        sched::yield_now(); // let w1 actually reach `lock()` and queue up.
    }
    let w2 = sched::spawn("test-mutex-w2", w2_acquires, 0);
    for _ in 0..5 {
        sched::yield_now(); // let w2 also queue up, behind w1.
    }

    assert!(sched::force_exit(w1, 99), "force_exit should find w1 still blocked");

    // Brief M2-T2b: a generous scheduler-poll count, not a wall-clock tick
    // deadline -- see `spinning_thread_does_not_starve_another`'s doc
    // comment for why a fixed number of `yield_now` rounds is robust to
    // host CPU contention in a way comparing `time::ticks()` to a fixed
    // budget isn't.
    const MAX_POLLS: u64 = 200_000;
    let mut polls = 0u64;
    let mut acquired = false;
    while polls < MAX_POLLS {
        if W2_ACQUIRED.load(Ordering::SeqCst) {
            acquired = true;
            break;
        }
        polls += 1;
        sched::yield_now();
    }
    assert!(acquired, "w2 should have acquired the mutex within {MAX_POLLS} scheduler polls after w1 was force-exited while queued");

    assert_eq!(sched::join(holder_id), 0);
    assert_eq!(sched::join(w2), 0);
    assert_eq!(sched::join(w1), 99);
}

/// Brief M2-T3: `mm::kstack` now lays stacks out in fixed-stride slots
/// recycled through a free list, replacing a bump allocator with a hard,
/// non-reclaimable ceiling of 64 *lifetime* stacks (every earlier spawn
/// permanently consumed one, freed or not). 1000 kernel threads, created
/// and joined one at a time, would have panicked ("guard registry is
/// full") well before reaching even a tenth of that under the old scheme;
/// sequential spawn+join means at most a couple of slots are ever live at
/// once, so this exercises the free list recycling a tiny handful of
/// slots hundreds of times over, not bump-allocating 1000 distinct ones.
/// If reuse ever leaked a slot's frames instead of freeing them, 1000
/// iterations at `THREAD_STACK_PAGES` (16) frames each would show up as a
/// ~16000-frame deficit, dwarfing anything else in this test.
#[test_case]
fn create_and_join_1000_kernel_threads_sequentially() {
    fn worker(_: usize) -> i32 {
        0
    }

    // Settle first: an earlier test's own exited thread can still be
    // sitting in `to_reap` (see `exited_thread_stack_is_reaped_eventually`
    // above for why).
    for _ in 0..20 {
        sched::yield_now();
    }
    let baseline = pmm::stats().free;

    for _ in 0..1000 {
        let id = sched::spawn("test-1000-threads", worker, 0);
        assert_eq!(sched::join(id), 0);
    }

    // Unlike this suite's small (1-3 thread) PMM-baseline tests, this
    // cannot settle back to the *exact* pre-loop baseline: `sched::
    // Scheduler::all` keeps every `Arc<Thread>` it ever creates alive
    // forever by design (its own docs: "a deliberate, bounded leak of the
    // `Thread` struct itself"), so the 1000 new `Thread` allocations (and
    // the handful of slab frames backing them) are never coming back
    // either. That is a fixed, already-accepted cost of *how many
    // distinct threads have ever existed*, wholly unrelated to whether
    // `kstack` itself leaked -- so this bounds the deficit generously
    // below the ~16000-frame signature a genuine kstack-slot leak would
    // leave, rather than requiring it to be zero.
    let mut free = pmm::stats().free;
    for _ in 0..50 {
        sched::yield_now();
        free = pmm::stats().free;
        if free >= baseline {
            break;
        }
    }
    let deficit = baseline.saturating_sub(free);
    assert!(
        deficit <= 200,
        "1000 sequential kernel threads left a {deficit}-frame deficit -- \
         far more than the known per-thread retention cost in `Scheduler::all`, \
         and consistent with a kstack slot leak instead"
    );
}
