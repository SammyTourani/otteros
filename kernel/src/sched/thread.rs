//! The kernel thread control block (brief M2-T1).

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};

use crate::arch::x86_64::fpu::FxsaveArea;
use crate::mm::addr::FRAME_SIZE;
use crate::mm::kstack::KernelStack;
use crate::mm::vmm::AddressSpace;

use super::context;

/// Monotonically increasing thread id, assigned by `sched::spawn`/
/// `sched::init` under the scheduler lock. Never reused.
pub type ThreadId = u64;

/// Every kernel thread stack is this big (brief M2-T1), matching the
/// pre-scheduler boot stack (`BOOT_STACK_PAGES`, lib.rs) and the
/// per-vector IST stacks' general size class.
pub(crate) const THREAD_STACK_PAGES: usize = 64 * 1024 / FRAME_SIZE;

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
    /// This thread's saved user FPU/SSE state (DECISIONS.md D13), boxed so
    /// an ordinary kernel `Thread` -- still the overwhelming majority --
    /// doesn't pay for 512+ bytes it will never use. `Some` for every
    /// thread `new_user_ready` creates (they all enter ring 3 essentially
    /// immediately, see `context::user_trampoline`), `None` otherwise.
    fpu_state: Option<Box<FxsaveArea>>,
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
    ) -> Arc<Self> {
        // A thread with an address space (brief M2-T2) has entered -- or,
        // for a brand-new one, is about to enter -- ring 3, so it gets an
        // FPU save area up front; see `fpu_state`'s own docs for why an
        // ordinary kernel thread doesn't.
        let fpu_state = address_space.is_some().then(FxsaveArea::pristine);
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
            handoff: AtomicBool::new(false),
            address_space,
            fpu_state,
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
        Self::new(id, name, stack, 0, unused, 0, ThreadState::Running, None)
    }

    /// Builds a brand-new, not-yet-run thread on `stack` (which the
    /// caller allocated), `Ready` to be picked up by the scheduler:
    /// `context::build_initial_stack` seeds `stack` so that the first
    /// `switch::switch_to` into it lands in `context::trampoline`, which
    /// reads `entry`/`arg` back out via `sched::current()` and calls
    /// `entry(arg)`.
    pub(crate) fn new_ready(id: ThreadId, name: &'static str, stack: KernelStack, entry: ThreadEntry, arg: usize) -> Arc<Self> {
        let rsp = context::build_initial_stack(stack.top);
        Self::new(id, name, stack, rsp, entry, arg, ThreadState::Ready, None)
    }

    /// Like `new_ready`, but for a brand-new *user* thread (brief M2-T2):
    /// seeds `stack` so the first `switch::switch_to` into it lands in
    /// `context::user_trampoline` instead, and records `address_space` so
    /// `sched::schedule` activates it before ever resuming this thread.
    pub(crate) fn new_user_ready(id: ThreadId, name: &'static str, stack: KernelStack, address_space: AddressSpace) -> Arc<Self> {
        fn unused(_: usize) -> i32 {
            unreachable!("Thread::new_user_ready: a user thread's `entry` is a placeholder, never called -- see context::user_trampoline")
        }
        let rsp = context::build_initial_user_stack(stack.top);
        Self::new(id, name, stack, rsp, unused, 0, ThreadState::Ready, Some(address_space))
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

    /// A raw pointer to this thread's FPU save area, if it has one (see
    /// the field's own docs) -- `None` for an ordinary kernel thread.
    ///
    /// Like `rsp_ptr`, only ever valid to write through while this thread
    /// isn't the one currently running, with interrupts disabled
    /// (`sched::schedule`, immediately before/after `arch::x86_64::switch::
    /// switch_to`): see `unsafe impl Sync for Thread`'s docs for the
    /// identical reasoning, now covering a second field.
    pub(crate) fn fpu_state_ptr(&self) -> Option<*mut FxsaveArea> {
        self.fpu_state.as_deref().map(core::ptr::from_ref).map(<*const FxsaveArea>::cast_mut)
    }

    /// The saved stack pointer. See the `unsafe impl Sync for Thread`
    /// docs above for why this is only ever valid to call the way
    /// `sched::schedule` calls it.
    pub(crate) fn rsp(&self) -> u64 {
        // SAFETY: see `unsafe impl Sync for Thread`'s docs.
        unsafe { *self.rsp.get() }
    }
}
