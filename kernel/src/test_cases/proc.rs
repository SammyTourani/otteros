//! Process/ring-3 tests (brief M2-T2): each spawns one or two tiny
//! hand-assembled payloads (`test_cases::payloads`) as real processes and
//! observes them purely through `proc`'s own public API (`spawn_payload`,
//! `wait`, `kill`, `find`) -- exactly the same "call the real API, never
//! reach into internals" stance `test_cases::sched` already takes for
//! kernel threads.

use super::payloads;
use otteros_kernel::arch::x86_64::usermode;
use otteros_kernel::mm::addr::VirtAddr;
use otteros_kernel::mm::pmm;
use otteros_kernel::proc;
use otteros_kernel::sched;

/// `hello` writes its message via a real `write` syscall (reaching the
/// console *and* serial through the same path the kernel's own `kprintln!`
/// uses -- serial log inspection is the acceptance check, not this
/// assertion) and exits with the exact code it asked for.
#[test_case]
fn hello_writes_and_exits_with_code_7() {
    let pid = proc::spawn_payload("test-hello", payloads::HELLO);
    assert_eq!(proc::wait(pid), Some(7));
}

/// A read from unmapped address 0 kills the process (never the kernel) --
/// `[proc] pid .. killed: PAGE FAULT ...` on serial is the acceptance
/// check; here, the *kernel* surviving to run this test's own final
/// assertions (and every test after it) is the proof "the test continues"
/// held.
#[test_case]
fn fault_payload_is_killed_with_page_fault() {
    const FAULT_EXIT_CODE: i32 = 128 + 11; // SIGSEGV-like; see proc::fault.
    let pid = proc::spawn_payload("test-fault", payloads::FAULT);
    assert_eq!(proc::wait(pid), Some(FAULT_EXIT_CODE));
}

/// A ring-3 thread that never yields or calls a syscall still can't starve
/// the kernel (pure timer preemption, exactly like `test_cases::sched::
/// spinning_thread_does_not_starve_another`'s kernel-thread version), and
/// `proc::kill` ends it on demand.
///
/// Brief M2-T2b: counts scheduler dispatches (`Thread::dispatches`, bumped
/// every time `sched::schedule` picks this thread to run), not wall-clock
/// ticks against a fixed deadline. Under host CPU contention the same 50
/// `yield_now` iterations can take far more *real* time -- and therefore
/// far more LAPIC ticks, a hardware timer running at a roughly constant
/// real-time rate, independent of how slow the guest itself is running --
/// without this thread being starved at all; that mismatch is exactly
/// what made a `ticks() - start < N` check here flaky under load.
/// `Thread::ticks_run` (bumped only when the periodic timer IRQ happens to
/// land while a thread is `current`) turned out to be the wrong counter
/// for this: this thread's own turn is typically far shorter than the gap
/// between two ticks (it yields again almost immediately), so it can
/// legitimately go dozens of genuine dispatches without ever overlapping
/// one -- `dispatches` counts the scheduling event directly instead.
#[test_case]
fn spin_does_not_starve_kernel_and_kill_ends_it() {
    let pid = proc::spawn_payload("test-spin", payloads::SPIN);

    let dispatches_before = sched::current().dispatches();
    for _ in 0..50 {
        sched::yield_now();
    }
    let dispatches_after = sched::current().dispatches();
    assert!(
        dispatches_after > dispatches_before,
        "the test thread should keep getting scheduled (dispatches {dispatches_before} -> {dispatches_after}) \
         even though `spin` never yields or syscalls"
    );

    assert!(proc::kill(pid, 42), "kill should find the still-running `spin` process");
    assert_eq!(proc::wait(pid), Some(42));
}

/// `write`ing through a kernel-half pointer never touches kernel memory --
/// `usermem::copy_from_user` refuses it, `write` returns `-EFAULT`, and the
/// payload reports that exact value back as its own exit code.
#[test_case]
fn badptr_write_returns_efault_as_exit_code() {
    const EFAULT: i32 = 14;
    let pid = proc::spawn_payload("test-badptr", payloads::BADPTR);
    assert_eq!(proc::wait(pid), Some(-EFAULT));
}

/// An unassigned syscall number (999) is `-ENOSYS`, not a crash or a
/// silently-succeeding no-op.
#[test_case]
fn nosys_unknown_syscall_returns_enosys() {
    const ENOSYS: i32 = 38;
    let pid = proc::spawn_payload("test-nosys", payloads::NOSYS);
    assert_eq!(proc::wait(pid), Some(ENOSYS));
}

/// Touching memory far below the 64 KiB the process starts with (but still
/// inside the 16 MiB reservation) demand-grows the stack instead of
/// killing the process -- if it didn't, this would instead come back as
/// the fault-kill exit code (128 + 11), not 0.
#[test_case]
fn growstack_demand_grows_and_exits_cleanly() {
    let pid = proc::spawn_payload("test-growstack", payloads::GROWSTACK);
    assert_eq!(proc::wait(pid), Some(0));
}

/// `wait` returns the *exact* exit code a process asked for, for a
/// perfectly ordinary process distinct from every other test's own.
#[test_case]
fn wait_returns_exit_code_for_a_fresh_process() {
    let pid = proc::spawn_payload("test-wait", payloads::HELLO);
    let code = proc::wait(pid).expect("a just-spawned process should be waitable");
    assert_eq!(code, 7);
}

/// Two processes both have code mapped at the identical virtual address
/// (`usermode::ENTRY_RIP`, D18 -- every process's fixed load address), but
/// `translate` resolves each one to a *different* physical frame: no
/// process can observe (let alone corrupt) another's memory through a
/// shared-looking address.
#[test_case]
fn two_processes_same_vaddr_have_different_physical_frames() {
    let pid_a = proc::spawn_payload("test-vaddr-a", payloads::SPIN);
    let pid_b = proc::spawn_payload("test-vaddr-b", payloads::SPIN);
    let process_a = proc::find(pid_a).expect("just spawned");
    let process_b = proc::find(pid_b).expect("just spawned");

    let (phys_a, _, _) = process_a
        .address_space()
        .translate(VirtAddr::new(usermode::ENTRY_RIP))
        .expect("process A's code page should be mapped");
    let (phys_b, _, _) = process_b
        .address_space()
        .translate(VirtAddr::new(usermode::ENTRY_RIP))
        .expect("process B's code page should be mapped");
    assert_ne!(phys_a, phys_b, "two processes' identically-addressed code pages must be backed by different physical frames");

    assert!(proc::kill(pid_a, 0));
    assert!(proc::kill(pid_b, 0));
    assert_eq!(proc::wait(pid_a), Some(0));
    assert_eq!(proc::wait(pid_b), Some(0));
}

/// Settles pending reaping (mirrors `test_cases::sched::
/// exited_thread_stack_is_reaped_eventually`'s identical reasoning), then
/// returns the current PMM free-frame count as a baseline for a later
/// comparison.
///
/// Kernel-review round 3: an earlier version of this file tried to force
/// an *exact* later comparison by brute-forcing every shared kernel
/// collection (`sched::Scheduler::all`, the process registry, ...) past
/// its last one-time capacity growth with a dedicated warm-up test that
/// ran first. That test alone spawned far more threads/processes than
/// `mm::kstack::MAX_STACKS` (64) allows across a single test-binary
/// lifetime -- every `sched::spawn`/`spawn_user` call permanently consumes
/// one guard-registry slot that is *never* reclaimed, even once the
/// thread exits and its stack is freed (see `mm::kstack::register`'s own
/// docs) -- so it reliably panicked (`kstack: guard registry is full`)
/// long before any assertion below ever ran. With the whole suite's total
/// thread/process count already close to that ceiling, there is no spare
/// budget for a brute-force warm-up; callers below settle for "returns to
/// at least the baseline" (never a permanent per-test leak) rather than
/// bit-exact equality.
fn settled_pmm_baseline() -> usize {
    for _ in 0..20 {
        sched::yield_now();
    }
    pmm::stats().free
}

/// Every frame a process used -- its code page, its (possibly demand-
/// grown) stack pages, and its own PML4 -- comes back to the PMM once it
/// has exited and `wait` has collected it: no leaks.
#[test_case]
fn process_exit_frees_all_frames_back_to_pmm_baseline() {
    let baseline = settled_pmm_baseline();

    let pid = proc::spawn_payload("test-leak", payloads::HELLO);
    assert_eq!(proc::wait(pid), Some(7));

    let mut freed = false;
    for _ in 0..50 {
        sched::yield_now();
        if pmm::stats().free >= baseline {
            freed = true;
            break;
        }
    }
    assert!(freed, "expected every frame the process used to return to the PMM baseline after exit+wait");
    assert_eq!(pmm::stats().free, baseline);
}

/// A syscall from a brand-new process (B) works correctly right after a
/// completely different process (A) ran, was torn down, and its kernel
/// stack was reaped -- if per-thread `RSP0`/`kernel_rsp` weren't reset
/// correctly for B's own (freshly allocated) kernel stack, B's very first
/// `syscall` would corrupt memory or hang instead of completing cleanly.
#[test_case]
fn syscall_from_process_b_after_process_a_works() {
    let pid_a = proc::spawn_payload("test-ab-a", payloads::HELLO);
    assert_eq!(proc::wait(pid_a), Some(7));

    let pid_b = proc::spawn_payload("test-ab-b", payloads::HELLO);
    assert_eq!(proc::wait(pid_b), Some(7));
}

// --- kernel-review round 2: adversarial payloads --------------------------

const EINVAL: i32 = 22;
const EFAULT: i32 = 14;
const ENOMEM: i32 = 12;

/// `unmap` of a kernel-half address is rejected (`-EINVAL`), never even
/// attempted -- `Process::unmap` must reject the whole range before
/// touching a single page table entry.
#[test_case]
fn unmap_kernel_half_address_is_einval() {
    let pid = proc::spawn_payload("test-unmap-kernel", payloads::UNMAP_KERNEL_HALF);
    assert_eq!(proc::wait(pid), Some(EINVAL));
}

/// `unmap` of a user page that was never mapped is also `-EINVAL` (not a
/// silent no-op).
#[test_case]
fn unmap_never_mapped_page_is_einval() {
    let pid = proc::spawn_payload("test-unmap-unmapped", payloads::UNMAP_UNMAPPED);
    assert_eq!(proc::wait(pid), Some(EINVAL));
}

/// `read(0, ..)` into the process's own (read-only) code page fails
/// `-EFAULT` immediately -- and, critically, *returns at all* rather than
/// hanging forever waiting for a keypress nobody can act on (kernel-review
/// round 2: `sys_read` validates the destination is `WRITABLE` before
/// blocking).
#[test_case]
fn read_into_read_only_code_page_is_efault_not_a_hang() {
    let pid = proc::spawn_payload("test-read-own-code", payloads::READ_OWN_CODE);
    assert_eq!(proc::wait(pid), Some(EFAULT));
}

/// `read(0, ..)` into a never-mapped page also fails `-EFAULT` immediately.
#[test_case]
fn read_into_unmapped_page_is_efault() {
    let pid = proc::spawn_payload("test-read-unmapped", payloads::READ_UNMAPPED);
    assert_eq!(proc::wait(pid), Some(EFAULT));
}

/// `map_anon` of a length far beyond the 1 GiB per-call cap is rejected,
/// never an unbounded kernel allocation attempt or an assert.
#[test_case]
fn map_anon_huge_length_is_einval_or_enomem() {
    let pid = proc::spawn_payload("test-map-anon-huge", payloads::MAP_ANON_HUGE);
    let code = proc::wait(pid).expect("should be waitable");
    assert!(code == EINVAL || code == ENOMEM, "expected EINVAL or ENOMEM, got {code}");
}

/// `map_anon` a page, write to it (succeeds), `unmap` it, then touch it
/// again: the last access must fault (the mapping is genuinely gone) and
/// kill the process -- proving `unmap` actually removed the translation,
/// not just decremented some count -- while the kernel keeps running.
#[test_case]
fn map_anon_write_unmap_then_access_is_killed() {
    const FAULT_EXIT_CODE: i32 = 128 + 11;
    let pid = proc::spawn_payload("test-map-anon-wua", payloads::MAP_ANON_WUA);
    assert_eq!(proc::wait(pid), Some(FAULT_EXIT_CODE));
}

/// Setting `RFLAGS.NT` from ring 3 (`pushf`/`or`/`popf`) and then spinning
/// across several preemptions can never corrupt or crash the kernel --
/// RFLAGS is sanitised on every return to ring 3 (kernel-review round 2).
/// The process either keeps running until explicitly killed, or is felled
/// by some fault on its own -- either outcome is acceptable, as long as
/// the kernel (and this test thread) keeps making progress throughout.
///
/// Brief M2-T2b: see `spin_does_not_starve_kernel_and_kill_ends_it`'s doc
/// comment for why this counts `Thread::dispatches` (how many times
/// `sched::schedule` picked this thread) rather than comparing wall-clock
/// `time::ticks()` against a fixed deadline, or `Thread::ticks_run`
/// (unreliable for a thread whose own turn is shorter than a tick).
#[test_case]
fn setting_nt_flag_never_corrupts_the_kernel() {
    let pid = proc::spawn_payload("test-nt-spin", payloads::NT_SPIN);

    let dispatches_before = sched::current().dispatches();
    for _ in 0..50 {
        sched::yield_now();
    }
    let dispatches_after = sched::current().dispatches();
    assert!(
        dispatches_after > dispatches_before,
        "the test thread should keep getting scheduled (dispatches {dispatches_before} -> {dispatches_after}) \
         even with an NT-spinning ring-3 process alive"
    );

    // Whether it's still alive or already gone, `kill` (idempotent for an
    // already-exited target) plus `wait` must cleanly collect it either
    // way, with the kernel none the worse for it.
    proc::kill(pid, 42);
    assert!(proc::wait(pid).is_some());
}

/// FPU/SSE state never leaks between processes: `fpu_write` sets `xmm0` to
/// a distinctive pattern and yields (a real context switch saves it), then
/// a completely separate process (`fpu_read`) must see `xmm0` as zero, not
/// `fpu_write`'s pattern -- DECISIONS.md D13's per-thread FPU save/restore,
/// with a genuinely clean (`fninit`/`MXCSR` default, kernel-review round 2)
/// initial state.
#[test_case]
fn fpu_state_does_not_leak_between_processes() {
    let pid_a = proc::spawn_payload("test-fpu-write", payloads::FPU_WRITE);
    // Let A actually run (set xmm0, yield) a few times before B ever exists.
    for _ in 0..5 {
        sched::yield_now();
    }
    assert_eq!(proc::wait(pid_a), Some(0));

    let pid_b = proc::spawn_payload("test-fpu-read", payloads::FPU_READ);
    assert_eq!(proc::wait(pid_b), Some(0), "process B observed a nonzero xmm0 -- FPU state leaked across processes");
}

/// Spawns `payload`, lets it actually reach its blocking point (`READ_BLOCK`
/// registers in the keyboard's `READ_QUEUE`; `SLEEP_LONG` registers in
/// `sched`'s own sleep list), then kills and collects it.
fn spawn_block_kill_wait(name: &'static str, payload: &[u8]) {
    let pid = proc::spawn_payload(name, payload);
    // Give it several chances to actually reach the blocking point and
    // register itself as a waiter before killing it.
    for _ in 0..10 {
        sched::yield_now();
    }
    assert!(proc::kill(pid, 42), "kill should find the still-blocked/sleeping process");
    assert_eq!(proc::wait(pid), Some(42));
}

/// Killing a process blocked in `read(0)` (never going to unblock on its
/// own in this environment -- nothing presses a key) frees its kernel
/// stack immediately, rather than leaking it forever (kernel-review round
/// 2's reported leak fix) -- and doing it *again* doesn't leak anything
/// further either (proving `force_exit`'s stack reclaim, not some one-off
/// coincidence, is what's keeping this stable).
#[test_case]
fn kill_of_process_blocked_in_read_frees_its_kernel_stack() {
    let baseline = settled_pmm_baseline();

    spawn_block_kill_wait("test-read-block", payloads::READ_BLOCK);

    let mut freed = false;
    for _ in 0..50 {
        sched::yield_now();
        if pmm::stats().free >= baseline {
            freed = true;
            break;
        }
    }
    assert!(freed, "killing a Blocked process should free its kernel stack back to the PMM baseline");
    assert_eq!(pmm::stats().free, baseline);
}

/// Killing a `Sleeping` process (a 60-second `sleep_ms` nothing will ever
/// let elapse during a test) also frees its kernel stack immediately, and
/// removes it from the sleep list (so a later, coincidentally-reused
/// `wake_at` tick can never resurrect it).
#[test_case]
fn kill_of_sleeping_process_frees_its_kernel_stack() {
    let baseline = settled_pmm_baseline();

    spawn_block_kill_wait("test-sleep-long", payloads::SLEEP_LONG);

    let mut freed = false;
    for _ in 0..50 {
        sched::yield_now();
        if pmm::stats().free >= baseline {
            freed = true;
            break;
        }
    }
    assert!(freed, "killing a Sleeping process should free its kernel stack back to the PMM baseline");
    assert_eq!(pmm::stats().free, baseline);
}

// --- kernel-review round 3: process lifecycle races -----------------------

/// A kernel-thread entry point that just calls `proc::wait(pid)` and
/// reports the result as its own exit code (`-1` for `None`, which never
/// actually happens in the tests below since both always target a real,
/// currently-running process).
fn wait_and_report(arg: usize) -> i32 {
    proc::wait(arg as u64).unwrap_or(-1)
}

/// Two threads both `wait` on the very same pid: both must see the exact
/// same exit code, and -- the actual bug -- only *one* of them may ever
/// call `AddressSpace::destroy` on it. Before the fix, both would (`wait`
/// unconditionally destroyed after `join`), double-freeing the PML4 frame
/// and panicking (`pmm::free_frame`'s own double-free assert).
#[test_case]
fn two_waiters_on_same_pid_no_double_free() {
    let baseline = settled_pmm_baseline();

    let pid = proc::spawn_payload("test-two-waiters", payloads::SPIN);
    for _ in 0..5 {
        sched::yield_now(); // let it actually start spinning.
    }

    let t1 = sched::spawn("test-waiter-1", wait_and_report, pid as usize);
    let t2 = sched::spawn("test-waiter-2", wait_and_report, pid as usize);
    for _ in 0..5 {
        sched::yield_now(); // let both threads reach the blocking join() inside wait().
    }

    assert!(proc::kill(pid, 77));

    assert_eq!(sched::join(t1), 77, "both waiters should see the exact same exit code");
    assert_eq!(sched::join(t2), 77, "both waiters should see the exact same exit code");

    let mut freed = false;
    for _ in 0..50 {
        sched::yield_now();
        if pmm::stats().free >= baseline {
            freed = true;
            break;
        }
    }
    assert!(freed, "two waiters on the same pid should still return every frame to the PMM baseline");
    assert_eq!(pmm::stats().free, baseline);
}

/// A kernel-thread entry point that calls `proc::kill(pid, 55)` and
/// reports whether it found the process, as its own exit code.
fn killer(arg: usize) -> i32 {
    i32::from(proc::kill(arg as u64, 55))
}

/// Two threads both call `kill` on the very same, still-spinning process
/// at once. Before the fix, `kill` freed the address space *before*
/// marking the thread `Exited` (a window where a preemption could resume
/// it on half-demolished page tables) and had no guard against two
/// concurrent `free_user_space` calls on the same process at all -- either
/// could double-free a frame. The kernel must survive either way, and
/// every frame must still come back to the PMM baseline.
#[test_case]
fn concurrent_kills_on_spinning_process_are_safe() {
    let baseline = settled_pmm_baseline();

    let pid = proc::spawn_payload("test-kill-race", payloads::SPIN);
    for _ in 0..5 {
        sched::yield_now(); // let it actually start spinning (Ready/Running).
    }

    let k1 = sched::spawn("test-killer-1", killer, pid as usize);
    let k2 = sched::spawn("test-killer-2", killer, pid as usize);

    // Both killers must complete without panicking; each individually
    // reports "found it" (`kill` returns `true` even for whichever one
    // loses the `begin_exit` race -- it still forces the thread `Exited`,
    // it just doesn't *also* free the address space).
    assert_eq!(sched::join(k1), 1);
    assert_eq!(sched::join(k2), 1);

    assert_eq!(proc::wait(pid), Some(55));

    let mut freed = false;
    for _ in 0..50 {
        sched::yield_now();
        if pmm::stats().free >= baseline {
            freed = true;
            break;
        }
    }
    assert!(freed, "two concurrent kills on the same spinning process should still free every frame back to the PMM baseline");
    assert_eq!(pmm::stats().free, baseline);
}

/// Brief M2-T3: `mm::kstack` now lays stacks out in fixed-stride slots
/// recycled through a free list (replacing a bump allocator with a hard,
/// non-reclaimable ceiling of 64 *lifetime* stacks -- exactly what this
/// process-spawning-heavy milestone needs headroom for). 200 ring-3
/// processes, spawned and waited one at a time, exercise the *exact* same
/// slot a process's main thread's kernel stack gets recycled through
/// `proc::wait`'s teardown -> `sched::join`'s reap -> `kstack::free` path,
/// over and over -- if that reuse ever leaked a slot's frames instead of
/// freeing them, 200 iterations at `STACK_PAGES` (16) frames each would
/// show up as a ~3200-frame deficit, dwarfing anything else in this test.
#[test_case]
fn spawn_wait_200_ring3_processes_sequentially() {
    let baseline = settled_pmm_baseline();

    for i in 0..200 {
        let pid = proc::spawn_payload("test-200-procs", payloads::HELLO);
        assert_eq!(proc::wait(pid), Some(7), "process {i} should exit with HELLO's own code");
    }

    // Unlike this suite's small (1-3 process) PMM-baseline tests, this
    // cannot settle back to the *exact* pre-loop baseline: every one of
    // the 200 processes' main threads is a ring-3 thread, which
    // `sched::thread::Thread::new_user_ready` gives a boxed, 512-byte
    // `FxsaveArea` (DECISIONS.md D13) -- and `sched::Scheduler::all` keeps
    // every `Arc<Thread>` it ever creates alive forever by design (its own
    // docs: "a deliberate, bounded leak of the `Thread` struct itself"),
    // so those 200 boxes, and the handful of slab frames backing them,
    // are never coming back either. That is a fixed, already-accepted
    // cost of *how many distinct threads have ever existed*, wholly
    // unrelated to whether `kstack` itself leaked -- so this bounds the
    // deficit generously below the ~3200-frame signature a genuine
    // kstack-slot leak would leave, rather than requiring it to be zero.
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
        "200 sequential ring-3 processes left a {deficit}-frame deficit -- \
         far more than the known per-thread FxsaveArea retention cost, and \
         consistent with a kstack slot leak instead"
    );
}
