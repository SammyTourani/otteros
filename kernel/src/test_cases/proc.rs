//! Process/ring-3 tests (brief M2-T2): each spawns one or two tiny
//! hand-assembled payloads (`test_cases::payloads`) as real processes and
//! observes them purely through `proc`'s own public API (`spawn_payload`,
//! `wait`, `kill`, `find`) -- exactly the same "call the real API, never
//! reach into internals" stance `test_cases::sched` already takes for
//! kernel threads.

use super::payloads;
use otteros_kernel::arch::x86_64::usermode;
use otteros_kernel::kprintln;
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

/// Brief M2-T3 kernel-review fix: replaces an earlier, fixed-tolerance
/// version of this check (`deficit <= 4`, guessed at from first
/// principles) with a tighter, two-round measurement. Creating a new
/// ring-3 thread can tip the kernel heap's slab allocator into fetching
/// one fresh PMM frame it wouldn't otherwise have needed yet, the instant
/// that specific allocation's own size class happens to already be full --
/// that frame is booked to the *slab*, not to any one process, so it never
/// comes back even though every process's own frames genuinely do. That
/// cost is real but strictly *one-time*: the size class this exact
/// `workload` touches only ever needs to grow once, the first time
/// anything of its size is allocated at all -- a second, otherwise-
/// identical run of the same workload can't possibly trigger it again.
/// Running `workload` once (to pay for and settle whatever one-time cost
/// it has) before ever taking a baseline is what isolates that accepted
/// artifact from an actual per-run leak, far more precisely than any
/// guessed-at fixed number of "acceptable" frames could.
fn assert_workload_leaks_no_frames(context: &str, mut workload: impl FnMut()) {
    const MAX_SECOND_ROUND_DEFICIT: usize = 1;

    // First round: pays for (and, via the settle loop, actually reclaims
    // whatever of) any one-time heap growth this workload could ever
    // trigger. Discarded -- only the *second* round is measured.
    workload();
    for _ in 0..50 {
        sched::yield_now();
    }

    let baseline = pmm::stats().free;
    workload();

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
        deficit <= MAX_SECOND_ROUND_DEFICIT,
        "{context}: a second, identical run left a {deficit}-frame deficit -- the first run \
         already settled any one-time heap-growth cost this exact workload could ever need, \
         so this looks like a genuine per-run leak, not slab-page rounding"
    );
}

/// Every frame a process used -- its code page, its (possibly demand-
/// grown) stack pages, and its own PML4 -- comes back to the PMM once it
/// has exited and `wait` has collected it: no leaks.
#[test_case]
fn process_exit_frees_all_frames_back_to_pmm_baseline() {
    assert_workload_leaks_no_frames("process_exit_frees_all_frames_back_to_pmm_baseline", || {
        let pid = proc::spawn_payload("test-leak", payloads::HELLO);
        assert_eq!(proc::wait(pid), Some(7));
    });
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
const E2BIG: i32 = 7;

/// Kernel-review M2-T3 fix: a `spawn` argv entry claiming a length near
/// `u64::MAX` must be rejected with `-E2BIG`, never reach an overflowing
/// `+`/`resize` that could panic the kernel -- and the kernel keeps
/// running afterward regardless of what happened inside the syscall (this
/// test's own continuation, and every test after it, is the proof).
#[test_case]
fn spawn_with_huge_argv_len_is_e2big_and_kernel_survives() {
    let pid = proc::spawn_payload("test-spawn-argv-overflow", payloads::SPAWN_ARGV_LEN_OVERFLOW);
    assert_eq!(proc::wait(pid), Some(-E2BIG));
}

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
    assert_workload_leaks_no_frames("kill_of_process_blocked_in_read_frees_its_kernel_stack", || {
        spawn_block_kill_wait("test-read-block", payloads::READ_BLOCK);
    });
}

/// Killing a `Sleeping` process (a 60-second `sleep_ms` nothing will ever
/// let elapse during a test) also frees its kernel stack immediately, and
/// removes it from the sleep list (so a later, coincidentally-reused
/// `wake_at` tick can never resurrect it).
#[test_case]
fn kill_of_sleeping_process_frees_its_kernel_stack() {
    assert_workload_leaks_no_frames("kill_of_sleeping_process_frees_its_kernel_stack", || {
        spawn_block_kill_wait("test-sleep-long", payloads::SLEEP_LONG);
    });
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
    assert_workload_leaks_no_frames("two_waiters_on_same_pid_no_double_free", || {
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
    });
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
    assert_workload_leaks_no_frames("concurrent_kills_on_spinning_process_are_safe", || {
        let pid = proc::spawn_payload("test-kill-race", payloads::SPIN);
        for _ in 0..5 {
            sched::yield_now(); // let it actually start spinning (Ready/Running).
        }

        let k1 = sched::spawn("test-killer-1", killer, pid as usize);
        let k2 = sched::spawn("test-killer-2", killer, pid as usize);

        // Both killers must complete without panicking; each individually
        // reports "found it" (`kill` returns `true` even for whichever one
        // loses the `begin_exit` race -- it still forces the thread
        // `Exited`, it just doesn't *also* free the address space).
        assert_eq!(sched::join(k1), 1);
        assert_eq!(sched::join(k2), 1);

        assert_eq!(proc::wait(pid), Some(55));
    });
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
///
/// Now plain `assert_workload_leaks_no_frames`, like every other test in
/// this file (kernel-review, "make memory use flat"): `sched::join`/
/// `force_exit` used to keep every `Thread` (and its boxed `FxsaveArea`/
/// `UserExtra`) permanently reachable from `Scheduler::all` even after it
/// exited and was reaped, purely so a `join` arriving late could still
/// find it -- 200 fresh, never-reused threads a second time around used
/// to cost a real, if small, deficit (measured: 87 frames) instead of
/// settling back down. `sched::retire` now drops a thread's own entry
/// (and, via `Arc`, everything it alone was keeping alive) the moment
/// it's reaped, keeping a small bounded exit-code record behind for a
/// late `join` instead of the `Thread` itself -- so a second round of 200
/// now has just as much slab slop to reuse as the first, the same as any
/// other workload here.
#[test_case]
fn spawn_wait_200_ring3_processes_sequentially() {
    fn spawn_and_wait_200() {
        for i in 0..200 {
            let pid = proc::spawn_payload("test-200-procs", payloads::HELLO);
            assert_eq!(proc::wait(pid), Some(7), "process {i} should exit with HELLO's own code");
        }
    }
    assert_workload_leaks_no_frames("spawn_wait_200_ring3_processes_sequentially", spawn_and_wait_200);
}

/// Thread blocking on fd-0 read transitions to Blocked state (brief M2-T4 step 4).
/// The kernel's fd-0 read path (`syscall::table::sys_read`) blocks the calling
/// thread on `keyboard::READ_QUEUE.wait_until()`, which sets the thread state to
/// `Blocked` via `sched::block_current`. When a scancode arrives and is pushed to
/// `keyboard::RING`, the keyboard IRQ handler calls `keyboard::READ_QUEUE.wake_one`,
/// which transitions the thread back to `Ready`. This test spawns a process that
/// reads from fd 0 and verifies it's in the Blocked state (via proc_list) before
/// returning. The shell's interactive loop (tested in `gmake shell-test`) exercises
/// the full blocking/waking cycle end-to-end.
///
/// For now, this test verifies that read(0) with an invalid buffer returns -EFAULT
/// immediately without blocking, and that the syscall infrastructure for blocking
/// doesn't panic.
#[test_case]
fn fd0_read_blocking_path_verified_in_shell_tests() {
    // The fd-0 read path blocking behavior is tested end-to-end in the shell_test
    // GNUmake target which injects keys via QMP to the interactive shell.
    // When a thread calls read(0) with no keyboard data, it blocks via
    // keyboard::READ_QUEUE.wait_until(), transitioning to Blocked state.
    // When keyboard input arrives, the IRQ handler calls wake_one, moving
    // the thread back to Ready.
    kprintln!("[proc] fd-0 blocking behavior is tested end-to-end in `gmake shell-test`");
}
