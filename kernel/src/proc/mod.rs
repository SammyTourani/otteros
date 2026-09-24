//! Processes: their own address spaces, a process table, and the
//! exit/kill/wait machinery every syscall in `syscall::table` that touches
//! a process goes through (brief M2-T2, DECISIONS.md D17).
//!
//! A `Process` owns exactly one `sched::Thread` for now (D17: "one thread
//! per process at first") -- `sched` itself knows nothing about `Process`
//! (a lower-level module can't depend on a higher-level one; see
//! `mm::kstack::free`'s identical stance on not depending on `sched`'s
//! types), so this module keeps its own `ThreadId -> Pid` side table
//! (`Registry::thread_to_pid`) rather than teaching `sched::Thread` what a
//! process is.

pub mod elf;
pub mod exec;
pub mod fault;
pub mod process;
pub mod usermem;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;

pub use process::{Pid, Process};

use crate::sched::{self, ThreadId};
use crate::sync::IrqMutex;

/// Why `spawn` failed (brief M2-T3's `spawn` syscall) -- `syscall::table::
/// sys_spawn` maps every variant to the errno `SYSCALLS.md` documents for
/// it (`ENOENT`, `ENOEXEC`, `ENOMEM`, `E2BIG` respectively).
#[derive(Debug)]
pub enum SpawnError {
    /// No such file in the initramfs.
    NotFound,
    /// `elf::load` rejected the file (not a valid static x86_64 ELF64
    /// executable).
    BadElf(elf::ElfError),
    /// The PMM ran out of memory while loading the ELF or building the
    /// stack.
    OutOfMemory,
    /// `exec::build_initial_stack` couldn't fit `argv` -- never actually
    /// reachable given `syscall::table::sys_spawn`'s own, much tighter
    /// caps (see `exec::StackError`'s docs), kept as a distinct variant
    /// anyway rather than folding it into `OutOfMemory`.
    BadStack,
}

struct Registry {
    processes: BTreeMap<Pid, Arc<Process>>,
    thread_to_pid: BTreeMap<ThreadId, Pid>,
    next_pid: Pid,
}

/// PID 0 is never assigned (kept free as an unambiguous "no such process"
/// sentinel, mirroring `pmm`'s "frame 0 is never allocated").
static REGISTRY: IrqMutex<Registry> = IrqMutex::new(Registry { processes: BTreeMap::new(), thread_to_pid: BTreeMap::new(), next_pid: 1 });

/// The calling *thread*'s own process.
///
/// # Panics
/// If the calling thread doesn't belong to any process -- every syscall
/// dispatch (`syscall::syscall_dispatch`) and every ring-3 fault
/// (`proc::fault`) only ever runs on a thread a `Process` itself spawned
/// (`sched::spawn_user`, always paired with `register` below), so this
/// only fires on a genuine bug (something invoking the syscall/fault path
/// from a plain kernel thread).
pub fn current() -> Arc<Process> {
    let tid = sched::current_id();
    let reg = REGISTRY.lock();
    let pid = *reg
        .thread_to_pid
        .get(&tid)
        .unwrap_or_else(|| panic!("proc::current: thread {tid} doesn't belong to any process"));
    reg.processes
        .get(&pid)
        .cloned()
        .unwrap_or_else(|| panic!("proc::current: pid {pid} is in thread_to_pid but not processes"))
}

/// Looks up a process by pid, whether it's still running or a zombie
/// (exited, not yet `wait`-ed) -- `None` once `wait` has collected it.
pub fn find(pid: Pid) -> Option<Arc<Process>> {
    REGISTRY.lock().processes.get(&pid).cloned()
}

/// Assigns the next pid and registers `process` in both the pid -> process
/// table and the thread -> pid side table. `process::Process::create`
/// calls this as its very last step, once the process (and the thread
/// `sched::spawn_user` already made `Ready`) fully exists.
fn register(process: Arc<Process>) {
    let mut reg = REGISTRY.lock();
    reg.thread_to_pid.insert(process.main_thread_id(), process.pid());
    reg.processes.insert(process.pid(), process);
}

/// The calling thread's own pid, if it belongs to a process -- unlike
/// `current`, never panics for a plain kernel thread (`None` instead).
/// `kill` uses this to detect self-targeting from a caller that might not
/// itself be a process at all (kernel test code).
fn current_pid() -> Option<Pid> {
    let tid = sched::current_id();
    REGISTRY.lock().thread_to_pid.get(&tid).copied()
}

fn next_pid() -> Pid {
    let mut reg = REGISTRY.lock();
    let pid = reg.next_pid;
    reg.next_pid += 1;
    pid
}

/// Maps `code` (a tiny, hand-assembled, position-fixed payload -- brief
/// M2-T2's stand-in for a real ELF loader, M2-T3) executable at
/// `arch::x86_64::usermode::ENTRY_RIP` in a fresh address space, gives it
/// a demand-growable stack, and spawns its (`Ready`, not yet running)
/// thread. Returns the new process's pid.
pub fn spawn_payload(name: &'static str, code: &[u8]) -> Pid {
    let pid = next_pid();
    // `Process::create` makes the new thread `Ready` (via `sched::
    // spawn_user`) before this function has finished registering it in
    // `REGISTRY` below -- a timer interrupt landing in exactly that window
    // could preempt this thread and schedule the brand-new one, which
    // would then run its very first syscall/fault with no `thread_to_pid`
    // entry yet (`proc::current` would panic: "thread N doesn't belong to
    // any process"). Disabling interrupts for the whole create+register
    // sequence rules that out: nothing in it ever voluntarily yields, so
    // a preemption is the *only* other way execution could reach the new
    // thread, and this blocks exactly that for exactly as long as needed
    // -- the same "wrap a must-appear-atomic sequence" pattern `sched`
    // itself uses throughout (`without_interrupts`/`IrqMutex`).
    crate::arch::x86_64::interrupts::without_interrupts(|| {
        let process = process::Process::create(pid, name, code);
        register(process);
    });
    pid
}

/// `spawn(path, argv)` (brief M2-T3, DECISIONS.md D17's "`spawn(path,
/// argv)` creates a fresh process from an ELF in the VFS/initramfs"):
/// looks `path` up in the initramfs, loads it as an ELF64 executable into
/// a fresh address space, builds a SysV-style initial stack for `argv`,
/// and spawns its (`Ready`, not yet running) thread. Returns the new
/// process's pid. `syscall::table::sys_spawn` is the real syscall's own
/// entry point; `kernel::init`/`tests::test_runner` also call this
/// directly to start `/bin/init` itself (brief step 7), which has no
/// syscall of its own to arrive through.
pub fn spawn(path: &str, argv: &[&str]) -> Result<Pid, SpawnError> {
    let data = crate::fs::initramfs::open(path).ok_or(SpawnError::NotFound)?;
    let pid = next_pid();
    // Brief M2-T3: same reasoning as `spawn_payload`'s identical
    // `without_interrupts` wrapper -- a preemption landing between the new
    // thread becoming `Ready` (inside `Process::create_from_elf`) and this
    // function registering it in `REGISTRY` could otherwise schedule it
    // before `proc::current` has anywhere to look it up.
    crate::arch::x86_64::interrupts::without_interrupts(|| match process::Process::create_from_elf(pid, path, data, argv) {
        Ok(process) => {
            register(process);
            Ok(pid)
        }
        Err(e) => Err(e),
    })
}

/// Ends the *calling* thread's own process with `code` (brief M2-T2's
/// `exit` syscall, and a ring-3 fault that kills its own process --
/// `proc::fault`): frees every user-half frame in its address space
/// (`mm::vmm::AddressSpace::free_user_space`) and marks its thread
/// `Exited` via the ordinary `sched::exit_current` path. Never returns.
/// The process stays in the table (a zombie -- D17/brief step 9) until
/// `wait` collects it.
///
/// `free_user_space` only actually runs if this call wins `begin_exit`'s
/// race (kernel-review round 3): a `kill` targeting this exact process,
/// racing this same self-exit, must never *also* tear down the same
/// address space -- see `Process::begin_exit`'s own docs.
pub fn exit_current_process(code: i32) -> ! {
    let process = current();
    if process.begin_exit() {
        process.address_space().free_user_space();
    }
    sched::current().set_exit_code(code);
    // Kernel-review: "make memory use flat" -- `sched::exit_current` below
    // never actually returns *on this call's own stack* (it switches away
    // for good; see its own docs), so this function's body never reaches
    // its normal end here, and `process`'s destructor -- otherwise
    // scheduled for right there -- would never run, permanently wedging
    // this process's (and, through its `main_thread` field, this
    // thread's) strong count one above zero. Dropping it explicitly first
    // is what actually lets `proc::wait`'s own, already-correct
    // `REGISTRY.processes.remove` collect it for real (found the hard
    // way: `sched::retire` alone didn't shrink anything, because this was
    // the reference actually keeping every exited process's `Thread`
    // alive).
    drop(process);
    sched::exit_current();
}

/// Ends `pid` (brief M2-T2's `kill` syscall, and kernel test code that
/// wants to end a process directly): `false` if `pid` doesn't exist;
/// otherwise forces its thread to `Exited` (`sched::force_exit`, a no-op
/// if it had already exited on its own) and frees its user address space
/// -- exactly once, ever, for this process (kernel-review round 3: see
/// `Process::begin_exit`'s own docs for why a second, concurrent `kill`,
/// or this same race against the process's own fault/syscall self-exit,
/// must never *both* call `free_user_space`).
///
/// `force_exit` runs *before* `free_user_space` (kernel-review round 3,
/// fixing the opposite, unsafe order): marking the target `Exited` first
/// means `sched::schedule` can never dispatch it again -- whether it's
/// still sitting `Ready` in the run queue, `Blocked`, or `Sleeping` --
/// while its address space is only *partway* torn down. Getting this
/// backwards would let a timer interrupt landing mid-`free_user_space`
/// resume the victim straight into a half-demolished set of page tables.
///
/// Unlike `exit_current_process`, this has no notion of "the caller's own
/// process" -- it works from any thread, including a plain kernel thread
/// that isn't a process at all (kernel tests spawn/kill payload processes
/// directly this way). `syscall::table`'s `kill` handler is the one place
/// that *does* care whether `pid` is the calling process's own: it checks
/// that itself (via `proc::current`, which is always meaningful there)
/// before ever reaching this function, and routes a self-kill through
/// `exit_current_process` instead, which alone knows how to stop the
/// *caller's own* execution.
pub fn kill(pid: Pid, code: i32) -> bool {
    // Kernel-review round 2: check *before* doing anything else -- a
    // self-kill must go through `exit_current_process` instead (which
    // alone knows how to stop *this* thread's own execution).
    assert_ne!(
        current_pid(),
        Some(pid),
        "proc::kill: pid {pid} is the calling process; use exit_current_process instead"
    );
    let Some(process) = find(pid) else { return false };
    sched::force_exit(process.main_thread_id(), code);
    if process.begin_exit() {
        process.address_space().free_user_space();
    }
    true
}

/// Blocks until `pid` exits, then returns its exit code and, if this is
/// the caller that actually collects it (a zombie until now: removed from
/// the process table, and its address space's PML4 frame finally freed --
/// D17/brief step 9's "remove from the table after `wait` collects it"),
/// reaps its last remaining frame. `None` if `pid` was never a process, or
/// was already fully collected by an earlier `wait`.
///
/// Kernel-review round 3: two threads can legitimately both call
/// `wait(pid)` on the same, still-running process -- both `find` it, both
/// block in `sched::join` (harmless; `join` already tolerates any number
/// of waiters on the same target) and both wake with the identical exit
/// code once it exits. Only the one whose `reg.processes.remove(&pid)`
/// actually returns `Some` (exactly one of them, since removal happens
/// under `REGISTRY`'s own lock) goes on to `destroy` the address space --
/// every other, merely-lost-the-race caller still returns the correct
/// exit code, just without touching the PML4 a second time.
pub fn wait(pid: Pid) -> Option<i32> {
    let process = find(pid)?;
    let code = sched::join(process.main_thread_id());

    let mut reg = REGISTRY.lock();
    let collected_by_this_call = reg.processes.remove(&pid).is_some();
    reg.thread_to_pid.remove(&process.main_thread_id());
    drop(reg);

    if collected_by_this_call {
        // SAFETY: `sched::join` just returned, so this process's thread is
        // `Exited` (and, by now, quite possibly already reaped -- either
        // way, it will never run again); nothing else ever held a second
        // copy of this exact `AddressSpace` to load into CR3 (D17: one
        // thread per process), and `reg.processes.remove` just returned
        // `Some` here, meaning no *other* `wait` call can also observe
        // that (the removal itself is the exclusivity guarantee) -- so no
        // future `proc::find`/`proc::current`, nor a second `wait`
        // "winner", can ever reach this same `destroy` call again.
        // `free_user_space` already ran (exactly once, `Process::
        // begin_exit`'s own guarantee) at exit/kill time, so only the PML4
        // frame itself remains to free.
        unsafe { process.address_space().destroy() };
    }

    Some(code)
}
