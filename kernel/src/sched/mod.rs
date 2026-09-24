//! Kernel threads and the preemptive round-robin scheduler (brief M2-T1):
//! the substrate every process runs on from M2 onward.
//!
//! # Design
//! One global `Scheduler` (`SCHED`, an `IrqMutex<Option<Scheduler>>`,
//! same "`None` until `init`" shape as `mm::pmm`/`mm::heap`) owns the
//! ready queue, the sleep list, the currently-running thread, the idle
//! thread, and a small registry of every thread ever spawned (`all`, for
//! `join`/introspection).
//!
//! Every public entry point that mutates scheduler state goes through
//! `with_sched`, which disables interrupts for the critical section --
//! the same reason `sync::IrqMutex` exists everywhere else in this
//! kernel: a timer tick landing while this core already holds `SCHED`
//! must not be able to try taking it again from `preempt_if_needed` and
//! spin forever against itself.
//!
//! `schedule()` is the one place a context switch actually happens. Its
//! precondition -- interrupts already disabled -- holds for both callers:
//! the IRQ path (`preempt_if_needed`, called after EOI, already running
//! with interrupts off because it's inside an interrupt gate) and every
//! voluntary path (`yield_now`, `sleep_ms`, `block_current`, `exit_current`),
//! which wrap themselves in `arch::x86_64::interrupts::without_interrupts`.
//! That wrapper is what makes the switch itself safe to reason about:
//! `without_interrupts(|| { ...; schedule(); })` disables interrupts,
//! calls `schedule()`, and -- once this exact thread is switched back in,
//! possibly much later -- restores interrupts to whatever they were on
//! entry, using a `was_enabled` local that lives on *this* stack frame
//! and is therefore preserved correctly across the switch. `schedule()`
//! itself never touches RFLAGS.IF at all: it releases `SCHED`'s lock
//! *before* calling `arch::x86_64::switch::switch_to` (a thread must be
//! able to re-take that lock once it resumes), but interrupts stay off
//! the whole time regardless, because nothing between the unlock and the
//! switch (or after resuming) ever calls `sti`. A thread resumed via
//! preemption instead naturally gets interrupts back via its own
//! trap frame's `iretq`, once it unwinds back out through
//! `irq::dispatch`/`trap_dispatch`/`interrupts::common_stub`.
//!
//! Reaping (brief M2-T1: "a thread must never free its own stack"): an
//! exiting thread's *own* final `schedule()` call queues it in `to_reap`
//! -- never processed by that same call, only by the *next* call to
//! `schedule()` (on some other thread, since an exited thread never calls
//! `schedule()` again) -- which frees its stack before doing anything
//! else.

pub mod mutex;
pub mod sleep;
pub mod waitqueue;

mod context;
mod idle;
mod runqueue;
mod thread;

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub use mutex::{Mutex, MutexGuard, Semaphore};
pub use sleep::sleep_ms;
pub use thread::{Thread, ThreadEntry, ThreadId, ThreadState};
pub use waitqueue::WaitQueue;

use runqueue::RunQueue;
use sleep::SleepEntry;
use thread::THREAD_STACK_PAGES;

use crate::arch::x86_64::interrupts::without_interrupts;
use crate::arch::x86_64::{fpu, gdt, percpu, switch};
use crate::mm::kstack::{self, KernelStack};
use crate::mm::vmm::{self, AddressSpace};
use crate::sync::IrqMutex;
use crate::{kprintln, time};

/// Upper bound on threads *concurrently* sitting in the ready queue at
/// once (brief M2-T1's design cautions: the ready queue's backing storage
/// is reserved to this capacity up front so pushing onto it, including
/// from IRQ context, never allocates). Generous headroom over anything
/// this kernel's tests ever run concurrently.
const MAX_THREADS: usize = 64;

/// Ticks a thread runs before `on_timer_tick` requests a reschedule
/// (DECISIONS.md D19: 1 kHz tick, 10 ms timeslice).
const TIMESLICE_TICKS: u64 = 10;

/// Bound on how many retired threads' exit codes `retire` keeps once
/// they're no longer in `all` (kernel-review: "make memory use flat").
/// Generous relative to how many threads could plausibly still have a
/// `join`/`force_exit` call outstanding against them by the time this
/// many *more* have since been retired -- a `join` that arrives after its
/// own target ages out of this ring (vs. one that arrives after ordinary
/// reaping, which this fix explicitly supports) is the same "no such
/// thread" panic `join` already gives for an id that was never spawned at
/// all; nothing in this kernel calls `join` anywhere near that late.
const MAX_RETIRED: usize = 128;

static NEED_RESCHED: AtomicBool = AtomicBool::new(false);
static TIMESLICE_REMAINING: AtomicU64 = AtomicU64::new(TIMESLICE_TICKS);

/// Threads parked in `join`, waiting for *some* thread to exit; woken
/// (all of them -- each re-checks its own specific target) every time any
/// thread does.
static JOIN_WAITQUEUE: WaitQueue = WaitQueue::new();

struct Scheduler {
    current: Arc<Thread>,
    idle: Arc<Thread>,
    ready: RunQueue,
    sleeping: Vec<SleepEntry>,
    /// Exited threads whose stack the *next* call to `schedule` (never
    /// this thread's own) should free. See the module docs.
    to_reap: Vec<Arc<Thread>>,
    /// Every thread that's currently alive in some form -- `Ready`,
    /// `Running`, `Blocked`, `Sleeping`, or `Exited` but not yet
    /// reaped -- so `join`/introspection can find it by id. `retire`
    /// removes an entry the moment its kernel stack is freed (kernel-
    /// review: "make memory use flat" -- this used to keep every thread
    /// ever spawned forever, an unbounded leak of the `Thread` struct
    /// itself, its boxed `FxsaveArea`/`UserExtra`, and everything else an
    /// `Arc<Thread>` clone here kept alive); see `retired` for how a
    /// `join`/`force_exit` call arriving after that still gets the right
    /// answer.
    all: Vec<Arc<Thread>>,
    /// Exit codes for threads `retire` has already removed from `all`,
    /// oldest-retired-first, capped at `MAX_RETIRED` (see its docs).
    retired: VecDeque<(ThreadId, i32)>,
    next_id: ThreadId,
}

static SCHED: IrqMutex<Option<Scheduler>> = IrqMutex::new(None);

fn with_sched<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    without_interrupts(|| {
        let mut guard = SCHED.lock();
        let sched = guard.as_mut().expect("sched::init was never called");
        f(sched)
    })
}

/// Turns the calling (boot) context into thread 0 ("main") and brings up
/// the idle thread. Must run once, after `mm::heap::init` (needs `alloc`)
/// and `mm::kstack::init_boot_stack` (this *is* that stack), and before
/// interrupts are ever enabled.
pub fn init(boot_stack: KernelStack) {
    let main = Thread::new_boot(0, "main", boot_stack);

    let idle_stack = kstack::allocate(THREAD_STACK_PAGES);
    let idle = Thread::new_ready(1, "idle", idle_stack, idle::idle_main, 0);

    let sched = Scheduler {
        current: main.clone(),
        idle: idle.clone(),
        ready: RunQueue::with_capacity(MAX_THREADS),
        sleeping: Vec::with_capacity(MAX_THREADS),
        to_reap: Vec::with_capacity(MAX_THREADS),
        all: alloc::vec![main, idle],
        retired: VecDeque::with_capacity(MAX_RETIRED),
        next_id: 2,
    };
    *SCHED.lock() = Some(sched);

    kprintln!("[sched] ready");
}

/// The currently-running thread.
pub fn current() -> Arc<Thread> {
    with_sched(|s| s.current.clone())
}

/// The currently-running thread's id.
pub fn current_id() -> ThreadId {
    current().id
}

/// Creates a new `Ready` kernel thread and returns its id. It becomes
/// eligible to run the next time any thread calls `schedule` (via
/// `yield_now`, blocking, sleeping, or simply the next timer tick).
///
/// # Panics
/// If the ready queue's reserved capacity (`MAX_THREADS`) would be
/// exceeded -- see `runqueue::RunQueue::push_back`.
pub fn spawn(name: &'static str, entry: ThreadEntry, arg: usize) -> ThreadId {
    let stack = kstack::allocate(THREAD_STACK_PAGES);
    with_sched(|s| {
        let id = s.next_id;
        s.next_id += 1;
        let thread = Thread::new_ready(id, name, stack, entry, arg);
        s.all.push(thread.clone());
        s.ready.push_back(thread);
        id
    })
}

/// Like `spawn`, but for a brand-new *user* thread (brief M2-T2): it
/// becomes `Ready` exactly the same way, except its first-ever `switch_to`
/// lands in `context::user_trampoline`, which enters ring 3 at
/// `(user_entry, user_rsp)` in `address_space` instead of calling an
/// ordinary `ThreadEntry` (brief M2-T3: generalised from a fixed pair to
/// per-thread values, so a real `spawn`ed ELF's own entry point and
/// argv-laden stack pointer work the same way the M2-T2 payload path's
/// constants always did). `proc::process::Process::create`/
/// `create_from_elf` are the only intended callers.
pub fn spawn_user(name: &'static str, address_space: AddressSpace, user_entry: u64, user_rsp: u64) -> ThreadId {
    let stack = kstack::allocate(THREAD_STACK_PAGES);
    with_sched(|s| {
        let id = s.next_id;
        s.next_id += 1;
        let thread = Thread::new_user_ready(id, name, stack, address_space, user_entry, user_rsp);
        s.all.push(thread.clone());
        s.ready.push_back(thread);
        id
    })
}

/// Looks up a thread by id, whether it's currently running, ready,
/// blocked, sleeping, or already exited but not yet reaped. `None` once
/// it's been reaped and `retire`d, even though it did genuinely exist --
/// `join`/`force_exit` fall back to `Scheduler::retired` for that case
/// instead of using this directly.
pub fn find(id: ThreadId) -> Option<Arc<Thread>> {
    with_sched(|s| s.all.iter().find(|t| t.id == id).cloned())
}

/// Removes `thread` from `all` and records its exit code in `retired`
/// (kernel-review: "make memory use flat"): the moment this call's own
/// caller drops its last local reference, `thread`'s `Arc` strong count
/// reaches zero and the `Thread` struct itself -- along with its boxed
/// `FxsaveArea`/`UserExtra` and every other per-thread allocation -- is
/// actually freed, rather than staying reachable forever through `all`.
/// Called exactly once per thread, at the same point its kernel stack is
/// freed (`schedule`'s `to_reap` loop, or `force_exit`'s own immediate-
/// reap branch) -- by then nothing will ever schedule `thread` again, so
/// nothing needs `find` to still see it either.
fn retire(s: &mut Scheduler, thread: &Arc<Thread>) {
    s.all.retain(|t| t.id != thread.id);
    if s.retired.len() >= MAX_RETIRED {
        s.retired.pop_front();
    }
    s.retired.push_back((thread.id, thread.exit_code()));
}

/// How many threads are currently sitting in the ready queue (test/
/// introspection only -- does not include `current` or `idle`).
pub fn ready_len() -> usize {
    with_sched(|s| s.ready.len())
}

/// Voluntarily gives up the CPU. The calling thread stays `Ready` and is
/// requeued (round-robin) -- `schedule` does this itself once it sees the
/// outgoing thread is still `Running`.
pub fn yield_now() {
    without_interrupts(schedule);
}

/// What `id` resolved to, the moment `join`/`force_exit` looked it up:
/// still a live entry in `all`, or already `retire`d (with its exit code
/// captured right there, under the same lock, so nothing can retire it a
/// second time -- or evict it from `retired` -- in between).
enum Lookup {
    Live(Arc<Thread>),
    Retired(i32),
}

fn lookup(id: ThreadId) -> Option<Lookup> {
    with_sched(|s| {
        if let Some(t) = s.all.iter().find(|t| t.id == id).cloned() {
            Some(Lookup::Live(t))
        } else {
            s.retired.iter().find(|(rid, _)| *rid == id).map(|&(_, code)| Lookup::Retired(code))
        }
    })
}

/// Blocks the current thread until `id` has exited, then returns its exit
/// code. Correct even if `id` had already exited *and been reaped* by the
/// time this is called (kernel-review: "make memory use flat" -- `id`'s
/// exit code was captured in `Scheduler::retired` at that point, see
/// `retire`) -- this doesn't need to still find a live `Thread` to answer
/// that case, only `Scheduler::retired`'s own small, bounded record.
///
/// # Panics
/// If `id` was never spawned, or was retired long enough ago to have aged
/// out of `Scheduler::retired`'s bound (`MAX_RETIRED`) -- nothing in this
/// kernel calls `join` anywhere near that late after a real spawn.
pub fn join(id: ThreadId) -> i32 {
    match lookup(id).unwrap_or_else(|| panic!("sched::join: no such thread {id}")) {
        Lookup::Live(thread) => {
            JOIN_WAITQUEUE.wait_until(|| thread.state() == ThreadState::Exited);
            thread.exit_code()
        }
        Lookup::Retired(code) => code,
    }
}

/// Ends the current thread. Never returns: its own state becomes
/// `Exited`, and (see the module docs) its final `schedule()` call always
/// switches to some *other* thread.
pub fn exit_current() -> ! {
    without_interrupts(|| {
        current().set_state(ThreadState::Exited);
        // Wake every `join` waiter (each re-checks its own specific
        // target against `ThreadState::Exited`; brief M2-T1's "re-checks
        // under the lock" `WaitQueue` design tolerates the ones this
        // wasn't meant for as ordinary spurious wake-ups).
        JOIN_WAITQUEUE.wake_all();
        schedule();
    });
    unreachable!("sched::exit_current: an exited thread's own schedule() call must never resume it")
}

/// Forcibly ends a thread *other than the current one* (brief M2-T2's
/// `kill`, via `proc::kill`): `false` if `id` was never spawned, `true`
/// otherwise (including when it had already exited -- idempotent, like
/// `wake`). Sets `code` as its exit code and marks it `Exited`; every
/// `join`/`WaitQueue` waiter is woken exactly as if it had exited on its
/// own.
///
/// A target still sitting in the ready queue is *not* removed from it
/// here (this kernel's `RunQueue` has no O(1) way to do that) -- instead,
/// `schedule`'s own pick-next loop recognises an already-`Exited` thread
/// the next time it's popped and reaps it there instead of ever resuming
/// it (see that function's docs), so it never actually runs again either
/// way. A target that's `Blocked`/`Sleeping` is likewise left in whatever
/// queue it's parked in: `wake`'s and `sleep::wake_due`'s own state checks
/// already refuse to resurrect an `Exited` thread, so it simply never
/// becomes `Ready` again -- though unlike the ready-queue case, nothing
/// then pops it back out to reap its stack; see `sleep::wake_due`'s docs
/// for the identical, already-accepted shape of gap.
///
/// # Panics
/// If `id` names the thread currently running *on this core* -- a caller
/// that wants to end its own thread must go through the ordinary exit
/// path (`exit_current`/`proc::exit_current_process`) instead, which alone
/// knows how to actually stop executing.
pub fn force_exit(id: ThreadId, code: i32) -> bool {
    // What this call found `id` to be, under one lock: no such thread
    // ever (`NotFound`); already retired -- reaped by an *earlier* call,
    // kernel-review: "make memory use flat" (`AlreadyRetired`, treated
    // exactly like the pre-existing "already exited" case below always
    // was: idempotent, `true`); or still in `all`, either already
    // `Exited` or just transitioned now (`Transitioned` -- `reap_now`
    // says whether *this* call must reclaim its kernel stack itself, see
    // below).
    enum Outcome {
        NotFound,
        AlreadyRetired,
        Transitioned { thread: Arc<Thread>, reap_now: bool },
    }
    let outcome = with_sched(|s| {
        let Some(thread) = s.all.iter().find(|t| t.id == id).cloned() else {
            return if s.retired.iter().any(|(rid, _)| *rid == id) { Outcome::AlreadyRetired } else { Outcome::NotFound };
        };
        assert!(
            !Arc::ptr_eq(&thread, &s.current),
            "sched::force_exit: {id} is the thread currently running on this core; use exit_current instead"
        );
        let prior_state = thread.state();
        if prior_state == ThreadState::Exited {
            return Outcome::Transitioned { thread, reap_now: false };
        }
        thread.set_exit_code(code);
        thread.set_state(ThreadState::Exited);
        // Kernel-review round 2 (the leak this fixes): a `Sleeping`
        // target is parked in `s.sleeping`, a plain `Vec` this module
        // owns directly -- unlike a `WaitQueue`'s waiters list (owned by
        // whichever *other* module's queue it happens to be, `sched`
        // itself has no way to reach into all of those), so it's removed
        // right here rather than left for `sleep::wake_due` to eventually
        // (and only if this thread's original timer ever elapses) notice
        // it's `Exited` and refuse to resurrect it.
        if prior_state == ThreadState::Sleeping {
            s.sleeping.retain(|entry| entry.thread.id != id);
        }
        let reap_now = matches!(prior_state, ThreadState::Blocked | ThreadState::Sleeping);
        Outcome::Transitioned { thread, reap_now }
    });
    let (thread, reap_now) = match outcome {
        Outcome::NotFound => return false,
        Outcome::AlreadyRetired => return true,
        Outcome::Transitioned { thread, reap_now } => (thread, reap_now),
    };
    JOIN_WAITQUEUE.wake_all();
    if reap_now {
        // A `Ready` victim is left for `schedule`'s own pick-next loop to
        // discover and reap (see its docs) -- it's sitting in `s.ready`,
        // which this function has no O(1) way to remove a specific entry
        // from. A `Blocked`/`Sleeping` one, though, is never going to
        // reach that loop at all (it isn't in the ready queue, and now
        // that it's `Exited`, `wake`/`sleep::wake_due` both refuse to
        // ever move it there) -- so its kernel stack is reclaimed right
        // here instead, the only place that will ever get the chance to.
        // Safe exactly because it's guaranteed to never run again
        // (`!owner_is_current`, just asserted above; `owner_exited`, just
        // set) -- freeing its stack doesn't require anything to first
        // remove the (now merely inert) `Arc<Thread>` reference still
        // sitting in whichever `WaitQueue`/the old sleep-list entry held
        // it; see `kstack::free`'s own contract.
        kstack::free(thread.stack(), true, false);
        with_sched(|s| retire(s, &thread));
    }
    true
}

/// Transitions the current thread to `Blocked` and lets `prepare` (called
/// with interrupts already disabled -- nothing else can run on this core
/// until either `prepare` returns `false` or `schedule` inside this call
/// actually switches away) register it wherever it needs to be found
/// again (typically a `WaitQueue`'s waiters list) before this thread
/// actually blocks. Returning `false` from `prepare` cancels the block
/// (the thread goes back to `Running` and this returns immediately) --
/// see `WaitQueue::wait_until`'s docs for why that matters.
pub(crate) fn block_current(prepare: impl FnOnce(Arc<Thread>) -> bool) {
    without_interrupts(|| {
        let thread = current();
        thread.set_state(ThreadState::Blocked);
        if prepare(thread.clone()) {
            schedule();
        } else {
            thread.set_state(ThreadState::Running);
        }
    });
}

/// Moves a `Blocked`/`Sleeping` thread back to the ready queue. A no-op
/// for a thread that's already `Ready`/`Running`/`Exited` (idempotent:
/// two racing wake-ups for the same thread must never queue it twice).
/// Safe to call from IRQ context: only ever enqueues (brief M2-T1's
/// design cautions).
///
/// kernel-review, M2-T1: if the CPU is currently sitting in `idle`, a
/// wake-up on its own doesn't get the newly-`Ready` thread onto the CPU
/// -- nothing but `idle`'s own preemption ever calls `schedule` again.
/// Setting `NEED_RESCHED` here makes `preempt_if_needed` (called right
/// after this returns, from the very same IRQ dispatch -- the keyboard's
/// or the timer's) switch away from `idle` immediately, rather than
/// waiting for the next unrelated timeslice expiry.
pub(crate) fn wake(thread: Arc<Thread>) {
    with_sched(|s| {
        if matches!(thread.state(), ThreadState::Blocked | ThreadState::Sleeping) {
            thread.set_state(ThreadState::Ready);
            s.ready.push_back(thread);
            if Arc::ptr_eq(&s.current, &s.idle) {
                NEED_RESCHED.store(true, Ordering::Relaxed);
            }
        }
    });
}

/// Performs at most one context switch and returns once this thread is
/// `Running` again -- possibly much later, after other threads have run
/// in between. **Precondition**: interrupts must already be disabled
/// (every caller above arranges this, or -- `preempt_if_needed` -- is
/// already running inside an interrupt gate).
fn schedule() {
    let (prev, next, next_top, next_rsp) = with_sched(|s| {
        // Free stacks queued by an *earlier* call to `schedule` -- never
        // this call's own outgoing thread (see the module docs): by
        // construction, an entry only ever reaches `to_reap` the instant
        // before its own final `schedule()` call switches away for good,
        // so no *other* call to `schedule()` (this one included) can ever
        // observe its own `s.current` sitting in `to_reap`.
        for exited in core::mem::take(&mut s.to_reap) {
            // kernel-review, M2-T1: `kstack::free` itself asserts both of
            // these (always actually `Exited`, never the currently-
            // running thread) rather than trusting this call site alone
            // -- passed explicitly, rather than the `Thread` itself, so
            // `mm::kstack` (lower-level than `sched`) doesn't need to
            // depend on `sched`'s own types.
            let is_exited = exited.state() == ThreadState::Exited;
            let is_current = Arc::ptr_eq(&exited, &s.current);
            kstack::free(exited.stack(), is_exited, is_current);
            // Kernel-review: "make memory use flat" -- this is the other
            // place (alongside `force_exit`'s own immediate-reap branch)
            // a thread's stack is ever freed, so it's the other place
            // `retire` removes it from `all` for good.
            retire(s, &exited);
        }

        let prev = s.current.clone();
        if !Arc::ptr_eq(&prev, &s.idle) {
            match prev.state() {
                ThreadState::Running => {
                    prev.set_state(ThreadState::Ready);
                    s.ready.push_back(prev.clone());
                }
                ThreadState::Exited => s.to_reap.push(prev.clone()),
                // Blocked/Sleeping: the caller already recorded `prev`
                // wherever it needs to be found again (a `WaitQueue`'s
                // waiters list, or the sleep list) -- nothing further to
                // do here.
                ThreadState::Blocked | ThreadState::Sleeping => {}
                ThreadState::Ready => unreachable!("schedule: the current thread can't already be Ready"),
            }
        }

        // Brief M2-T2: a thread `force_exit` marked `Exited` while it was
        // still sitting in the ready queue (e.g. `proc::kill` on a
        // process whose thread was merely preempted, never blocked) must
        // never actually be resumed -- reap it here instead, exactly like
        // a thread that reached `to_reap` by exiting on its own (see the
        // module docs), and keep looking for a real candidate.
        let next = loop {
            match s.ready.pop_front() {
                Some(candidate) if candidate.state() == ThreadState::Exited => {
                    s.to_reap.push(candidate);
                }
                Some(candidate) => break candidate,
                None => break s.idle.clone(),
            }
        };
        next.set_state(ThreadState::Running);
        next.record_dispatch();
        s.current = next.clone();
        let next_top = next.stack().top.as_u64();
        let next_rsp = next.rsp();
        (prev, next, next_top, next_rsp)
    });

    if Arc::ptr_eq(&prev, &next) {
        return; // Nothing else is runnable; stay on `prev` (usually `idle`).
    }

    // kernel-review, M2-T1: a fresh full timeslice for whoever is
    // actually dispatched now, every time -- not just when a timeslice
    // naturally expires (`on_timer_tick`'s own reset below still matters
    // for *that* case, since a switch doesn't always happen even then --
    // see `preempt_if_needed`). Without this, a thread reached via a
    // voluntary yield/wake inherited whatever was left of some unrelated
    // thread's countdown, sometimes getting preempted again almost
    // immediately.
    TIMESLICE_REMAINING.store(TIMESLICE_TICKS, Ordering::Relaxed);

    gdt::set_rsp0(next_top);
    percpu::set_kernel_rsp(next_top);
    percpu::set_current_thread(next.id);

    // Brief M2-T2 step 5's caution: switch CR3 only when the incoming
    // thread's address space actually differs from the outgoing one's --
    // comparing `AddressSpace`s directly (not just "does either have
    // one") is what lets two threads of the same process (a future
    // milestone; still 1:1 with a process for M2) share a reload-free
    // switch, while a `None` (ordinary kernel thread) always resolves to
    // the one, permanent kernel address space -- never a process's, which
    // can be torn down (and its PML4 frame freed, `proc::process::
    // Process`) the moment that process exits, so nothing should still be
    // "riding along" on it afterward.
    let prev_space = prev.address_space().unwrap_or_else(vmm::kernel_address_space);
    let next_space = next.address_space().unwrap_or_else(vmm::kernel_address_space);
    if next_space != prev_space {
        next_space.activate();
    }

    // DECISIONS.md D13: user FPU/SSE state is saved/restored per thread at
    // context switch, never on every trap -- a no-op pair of checks for
    // the (overwhelmingly common) case of two plain kernel threads
    // switching, neither of which ever has `fpu_state`.
    if let Some(ptr) = prev.fpu_state_ptr() {
        // SAFETY: `prev` is the thread switching away this exact call;
        // nothing else touches its FPU area while it isn't running (see
        // `Thread`'s `unsafe impl Sync` docs).
        unsafe { fpu::save(&mut *ptr) };
    }
    if let Some(ptr) = next.fpu_state_ptr() {
        // SAFETY: symmetric to `prev`'s save, immediately before `next`
        // resumes -- possibly straight into ring 3, where this state
        // becomes directly observable.
        unsafe { fpu::restore(&*ptr) };
    }

    // Kernel-review: "make memory use flat" -- `prev_rsp_ptr` is extracted
    // *before* dropping `prev`/`next` on purpose. When `prev` is exiting
    // for good, `switch_to` below never actually returns on this call's
    // own stack (it `ret`s into whatever *other* invocation of `schedule`
    // originally suspended `next`, possibly on some completely different
    // thread's stack) -- so anything still owned by this function's own
    // locals at the point of that call, these two `Arc<Thread>` clones
    // included, would otherwise never run its destructor. That silently
    // wedges `prev`'s (and `next`'s) strong count one above what it should
    // be, forever, which is exactly what let a `retire`d thread's `Thread`
    // struct (and its boxed `FxsaveArea`/`UserExtra`) keep leaking even
    // after `retire` removed its `Scheduler::all` entry (found the hard
    // way: this is why that fix alone didn't shrink the PMM deficit at
    // all). Dropping them explicitly first instead means the only
    // remaining reference to `prev` is whichever one `s.to_reap`/
    // `s.ready`/the sleep list/a `WaitQueue` already holds (every
    // reachable branch above registers one before ever calling `schedule`
    // at all), and to `next` is `s.current`'s own -- both already
    // guaranteed to outlive this exact call regardless.
    let prev_rsp_ptr = prev.rsp_ptr();
    drop(prev);
    drop(next);

    // SAFETY: `prev_rsp_ptr` points at the outgoing thread's own saved-rsp
    // slot; the `drop`s just above don't invalidate it (see their own
    // comment: some other reference always keeps the allocation alive).
    // `next_rsp` is either a stack `switch_to` itself previously suspended
    // (and which is not running anywhere else -- it just came off the
    // ready queue or is `idle`) or a freshly built one from `context::
    // build_initial_stack`; either way it points at a live, mapped,
    // guard-paged kernel stack. Interrupts are off for this entire
    // function (its own documented precondition), so neither stack can be
    // observed half-updated.
    unsafe { switch::switch_to(prev_rsp_ptr, next_rsp) };
    // Resumed here, possibly much later: just return. Whichever caller
    // above is responsible for restoring interrupts (if they need
    // restoring at all -- see the module docs).
}

/// Called from the IRQ dispatch path (`arch::x86_64::irq::dispatch`)
/// after EOI, with interrupts still disabled: switches away from the
/// interrupted thread if the timer tick has requested it. A no-op
/// (`NEED_RESCHED` unset) for every IRQ that isn't a timeslice-expiring
/// timer tick.
pub fn preempt_if_needed() {
    if NEED_RESCHED.swap(false, Ordering::Relaxed) {
        schedule();
    }
}

/// Called once per timer tick (`time::on_tick`, IRQ context): advances
/// the current thread's tick count, wakes any threads whose sleep has
/// elapsed, and -- once the timeslice has run out -- requests a
/// reschedule (actually performed by `preempt_if_needed`, after EOI).
/// Never allocates, never logs (brief M2-T1's design cautions).
pub(crate) fn on_timer_tick() {
    let now = time::ticks();
    with_sched(|s| {
        s.current.record_tick();
        let woke_any = sleep::wake_due(&mut s.sleeping, &mut s.ready, now);
        // kernel-review, M2-T1: see `wake`'s identical reasoning -- a
        // sleeper's timer elapsing while `idle` is current needs the
        // same immediate-reschedule nudge a `WaitQueue` wake-up gets.
        if woke_any && Arc::ptr_eq(&s.current, &s.idle) {
            NEED_RESCHED.store(true, Ordering::Relaxed);
        }
    });

    if TIMESLICE_REMAINING.fetch_sub(1, Ordering::Relaxed) <= 1 {
        TIMESLICE_REMAINING.store(TIMESLICE_TICKS, Ordering::Relaxed);
        NEED_RESCHED.store(true, Ordering::Relaxed);
    }
}
