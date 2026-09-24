//! The kernel thread control block (brief M2-T1).

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};

use crate::arch::x86_64::fpu::FxsaveArea;
use crate::mm::kstack::{self, KernelStack};
use crate::mm::vmm::AddressSpace;

use super::context;

/// Monotonically increasing thread id, assigned by `sched::spawn`/
/// `sched::init` under the scheduler lock. Never reused.
pub type ThreadId = u64;

/// Every kernel thread stack is this big (brief M2-T1). Brief M2-T3:
/// re-exports `kstack::STACK_PAGES` rather than repeating the
/// computation, since `kstack::allocate` now hard-requires every caller
/// to agree on one uniform size (its fixed-stride slot layout).
pub(crate) const THREAD_STACK_PAGES: usize = kstack::STACK_PAGES;

/// A thread's entry point: takes the opaque `arg` `spawn` was given and
/// returns an exit code (`sched::join` reads it back).
pub type ThreadEntry = fn(usize) -> i32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ThreadState {
    /// Sitting in the scheduler's ready queue (or, for `idle`, always
    /// eligible without ever actually being queued).
    Ready = 0,
    /// The thread the CPU is currently executing.
    Running = 1,
    /// Parked on a `WaitQueue` (a `Mutex`, `Semaphore`, `join`, or the
    /// blocking keyboard reader), waiting for `wake` to enqueue it.
    Blocked = 2,
    /// Parked on the sleep list, waiting for its wake tick.
    Sleeping = 3,
    /// Ran to completion; `exit_code()` is valid, its stack is queued (or
    /// already reaped) for reclamation.
    Exited = 4,
}

impl ThreadState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => ThreadState::Ready,
            1 => ThreadState::Running,
            2 => ThreadState::Blocked,
            3 => ThreadState::Sleeping,
            4 => ThreadState::Exited,
            _ => unreachable!("ThreadState::from_u8: invalid state byte {v}"),
        }
    }
}

/// One kernel thread. Always held as `Arc<Thread>` (brief M2-T1) -- the
/// scheduler's ready queue, sleep list, every `WaitQueue`'s waiters list,
/// and `sched::current()` all share the same underlying allocation rather
/// than copying thread state around.
pub struct Thread {
    pub id: ThreadId,
    pub name: &'static str,
    state: AtomicU8,
    stack: KernelStack,
    /// The saved stack pointer: valid to read/write only while this
    /// thread isn't the one currently running, and only ever touched
    /// with interrupts disabled (`sched::schedule`/`arch::x86_64::switch`)
    /// -- see the `unsafe impl Sync` below.
    rsp: UnsafeCell<u64>,
    entry: ThreadEntry,
    arg: usize,
    exit_code: AtomicI32,
    ticks_run: AtomicU64,
    /// How many times `sched::schedule` has picked this thread as `next`
    /// (brief M2-T2b), incremented once per selection regardless of how
    /// long the resulting run lasts. Unlike `ticks_run` (bumped only when
    /// the periodic ~1 kHz timer IRQ happens to land while this thread is
    /// `current`), this counts every dispatch even if a thread's own turn
    /// is far shorter than the gap between two ticks -- exactly the case
    /// for a thread that yields again almost immediately, which can
    /// legitimately go dozens of dispatches without ever overlapping a
    /// tick. Tests asserting "this thread kept getting scheduled" (not
    /// "this thread accumulated running time") should prefer this.
    dispatches: AtomicU64,
    /// Set by a direct hand-off (kernel-review, M2-T1: `sched::mutex::
    /// Mutex::unlock`/`Semaphore::release`) to tell *this specific*
    /// thread, once it wakes, that it was granted the lock/permit
    /// directly rather than needing to win it again -- so a different,
    /// not-yet-blocked caller can never barge in between the hand-off
    /// and this thread actually running. One flag per thread (not one
    /// per `Mutex`/`Semaphore`) so multiple concurrent hand-offs -- e.g.
    /// a `Semaphore` releasing to several distinct waiters in quick
    /// succession -- never collide with each other.
    handoff: AtomicBool,
    /// The address space to `activate()` whenever this thread becomes
    /// `current` and the outgoing thread's differs (brief M2-T2 step 5) --
    /// `None` for an ordinary kernel thread, which always runs in
    /// whatever address space is already active (every one of them maps
    /// the identical kernel half, D15, so it never actually matters which
    /// one). Set once, at thread creation (`new_user_ready`), never
    /// changed afterward.
    address_space: Option<AddressSpace>,
    /// Everything else a *user* thread alone needs, boxed together in one
    /// allocation (brief M2-T3) so an ordinary kernel `Thread` -- still the
    /// overwhelming majority -- doesn't pay for any of it. `Some` for
    /// every thread `new_user_ready` creates (they all enter ring 3
    /// essentially immediately, see `context::user_trampoline`), `None`
    /// otherwise.
    ///
    /// Kernel-review-worthy: `user_entry`/`user_rsp` used to be two plain
    /// `u64` fields directly on `Thread` -- correct, but it grew every
    /// `Thread` (kernel threads included) by 16 bytes, which can silently
    /// bump `size_of::<Thread>()` into a *larger* kernel-heap size class
    /// for `Arc<Thread>`'s own allocation. That shifts precisely when the
    /// heap needs a fresh slab page (a real PMM frame) -- for *every*
    /// thread this kernel ever creates, not just user ones -- which
    /// several `test_cases::sched`/`test_cases::proc` tests assert an
    /// *exact* PMM free-frame count around (found the hard way). Bundling
    /// the two extra fields into this already-conditional, already-boxed
    /// allocation instead keeps `size_of::<Thread>()` (and therefore every
    /// *kernel* thread's allocation class) completely unchanged; only
    /// user threads' own box grows, the same already-accepted category of
    /// cost as the FPU area itself.
    user: Option<Box<UserExtra>>,
}

/// A user thread's FPU/SSE save area plus where `context::user_trampoline`
/// should `enter_ring3` for it -- see the `Thread::user` field's own docs
/// for why these are bundled into one boxed allocation instead of three
/// separate `Thread` fields.
struct UserExtra {
    fpu: FxsaveArea,
    user_entry: u64,
    user_rsp: u64,
}

// SAFETY: `rsp`'s `UnsafeCell` (and, brief M2-T2, `fpu_state`'s contents,
// reached mutably through `fpu_state_ptr`'s raw pointer despite `&self`)
// are only ever read or written from `sched::schedule` (via `rsp()`/
// `rsp_ptr()`/`fpu_state_ptr()`) or `arch::x86_64::switch::switch_to`/
// `arch::x86_64::fpu::save` themselves, always with interrupts disabled
// and always for a thread that is *not* the one currently executing --
// this is a single-core kernel, so "interrupts disabled" already rules
// out any other code observing this core's state concurrently.
// `Arc<Thread>` needs `Thread: Sync` to be shared across the scheduler's
// data structures (the ready queue, sleep list, wait queues) even though
// nothing here is genuinely concurrent.
unsafe impl Sync for Thread {}

impl Thread {
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: ThreadId,
        name: &'static str,
        stack: KernelStack,
        rsp: u64,
        entry: ThreadEntry,
        arg: usize,
        state: ThreadState,
        address_space: Option<AddressSpace>,
        user_entry: u64,
        user_rsp: u64,
    ) -> Arc<Self> {
        // A thread with an address space (brief M2-T2) has entered -- or,
        // for a brand-new one, is about to enter -- ring 3, so it gets a
        // `UserExtra` (FPU save area + entry/rsp) up front; see the
        // `Thread::user` field's own docs for why an ordinary kernel
        // thread doesn't.
        let user = address_space
            .is_some()
            .then(|| Box::new(UserExtra { fpu: FxsaveArea::pristine_value(), user_entry, user_rsp }));
        Arc::new(Self {
            id,
            name,
            state: AtomicU8::new(state as u8),
            stack,
            rsp: UnsafeCell::new(rsp),
            entry,
            arg,
            exit_code: AtomicI32::new(0),
            ticks_run: AtomicU64::new(0),
            dispatches: AtomicU64::new(0),
            handoff: AtomicBool::new(false),
            address_space,
            user,
        })
    }

    /// Wraps the boot context (already running on `stack`, reached via
    /// `mm::kstack::switch_stack_and_call`, not `switch::switch_to`) as
    /// thread `id`. `rsp`/`entry`/`arg` are never read for this thread
    /// until the first time it is ever switched *away* from -- at which
    /// point `switch_to` overwrites `rsp` with wherever it actually left
    /// off -- so they start as inert placeholders.
    pub(crate) fn new_boot(id: ThreadId, name: &'static str, stack: KernelStack) -> Arc<Self> {
        fn unused(_: usize) -> i32 {
            unreachable!("Thread::new_boot: the boot thread's `entry` is a placeholder, never called")
        }
        Self::new(id, name, stack, 0, unused, 0, ThreadState::Running, None, 0, 0)
    }

    /// Builds a brand-new, not-yet-run thread on `stack` (which the
    /// caller allocated), `Ready` to be picked up by the scheduler:
    /// `context::build_initial_stack` seeds `stack` so that the first
    /// `switch::switch_to` into it lands in `context::trampoline`, which
    /// reads `entry`/`arg` back out via `sched::current()` and calls
    /// `entry(arg)`.
    pub(crate) fn new_ready(id: ThreadId, name: &'static str, stack: KernelStack, entry: ThreadEntry, arg: usize) -> Arc<Self> {
        let rsp = context::build_initial_stack(stack.top);
        Self::new(id, name, stack, rsp, entry, arg, ThreadState::Ready, None, 0, 0)
    }

    /// Like `new_ready`, but for a brand-new *user* thread (brief M2-T2):
    /// seeds `stack` so the first `switch::switch_to` into it lands in
    /// `context::user_trampoline` instead, and records `address_space` so
    /// `sched::schedule` activates it before ever resuming this thread.
    /// `user_entry`/`user_rsp` (brief M2-T3) are where `user_trampoline`
    /// enters ring 3 for this exact thread -- `proc::process::Process::
    /// create`'s fixed `usermode::ENTRY_RIP`/`usermode::USER_STACK_TOP`
    /// pair for the M2-T2 payload path, or a real ELF's own entry point
    /// and argv-laden initial stack pointer for `create_from_elf`.
    pub(crate) fn new_user_ready(
        id: ThreadId,
        name: &'static str,
        stack: KernelStack,
        address_space: AddressSpace,
        user_entry: u64,
        user_rsp: u64,
    ) -> Arc<Self> {
        fn unused(_: usize) -> i32 {
            unreachable!("Thread::new_user_ready: a user thread's `entry` is a placeholder, never called -- see context::user_trampoline")
        }
        let rsp = context::build_initial_user_stack(stack.top);
        Self::new(id, name, stack, rsp, unused, 0, ThreadState::Ready, Some(address_space), user_entry, user_rsp)
    }

    pub fn state(&self) -> ThreadState {
        ThreadState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub(crate) fn set_state(&self, state: ThreadState) {
        self.state.store(state as u8, Ordering::Relaxed);
    }

    pub fn stack(&self) -> KernelStack {
        self.stack
    }

    pub(crate) fn entry(&self) -> ThreadEntry {
        self.entry
    }

    pub(crate) fn arg(&self) -> usize {
        self.arg
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code.load(Ordering::Relaxed)
    }

    pub(crate) fn set_exit_code(&self, code: i32) {
        self.exit_code.store(code, Ordering::Relaxed);
    }

    pub fn ticks_run(&self) -> u64 {
        self.ticks_run.load(Ordering::Relaxed)
    }

    pub(crate) fn record_tick(&self) {
        self.ticks_run.fetch_add(1, Ordering::Relaxed);
    }

    /// How many times `sched::schedule` has picked this thread to run --
    /// see the `dispatches` field's own docs for how this differs from
    /// `ticks_run`.
    pub fn dispatches(&self) -> u64 {
        self.dispatches.load(Ordering::Relaxed)
    }

    pub(crate) fn record_dispatch(&self) {
        self.dispatches.fetch_add(1, Ordering::Relaxed);
    }

    /// Grants this thread a direct hand-off (see the `handoff` field's
    /// docs). Must be called before `sched::wake`ing it for that reason,
    /// while it is still safely parked.
    pub(crate) fn grant_handoff(&self) {
        self.handoff.store(true, Ordering::Release);
    }

    /// Consumes a pending direct hand-off, if one was granted: `true`
    /// exactly once per `grant_handoff` call, `false` otherwise.
    pub(crate) fn take_handoff(&self) -> bool {
        self.handoff.swap(false, Ordering::Acquire)
    }

    pub(crate) fn rsp_ptr(&self) -> *mut u64 {
        self.rsp.get()
    }

    /// The address space this thread runs in, if it's a user thread (see
    /// the field's own docs).
    pub(crate) fn address_space(&self) -> Option<AddressSpace> {
        self.address_space
    }

    /// Where `context::user_trampoline` should `enter_ring3` for this
    /// thread's very first dispatch (brief M2-T3) -- meaningless unless
    /// `address_space().is_some()`. Set once at construction and never
    /// mutated again, so (like `entry`/`arg`) this needs no special
    /// synchronisation despite `Thread`'s blanket `unsafe impl Sync`.
    pub(crate) fn user_entry_rsp(&self) -> (u64, u64) {
        self.user.as_deref().map_or((0, 0), |u| (u.user_entry, u.user_rsp))
    }

    /// A raw pointer to this thread's FPU save area, if it has one (see
    /// the `user` field's own docs) -- `None` for an ordinary kernel
    /// thread.
    ///
    /// Like `rsp_ptr`, only ever valid to write through while this thread
    /// isn't the one currently running, with interrupts disabled
    /// (`sched::schedule`, immediately before/after `arch::x86_64::switch::
    /// switch_to`): see `unsafe impl Sync for Thread`'s docs for the
    /// identical reasoning, now covering a second field.
    pub(crate) fn fpu_state_ptr(&self) -> Option<*mut FxsaveArea> {
        self.user.as_deref().map(|u| core::ptr::from_ref(&u.fpu).cast_mut())
    }

    /// The saved stack pointer. See the `unsafe impl Sync for Thread`
    /// docs above for why this is only ever valid to call the way
    /// `sched::schedule` calls it.
    pub(crate) fn rsp(&self) -> u64 {
        // SAFETY: see `unsafe impl Sync for Thread`'s docs.
        unsafe { *self.rsp.get() }
    }
}
