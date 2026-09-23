//! The in-kernel test harness binary.
//!
//! This exists as a *separate* bin target from `otteros-kernel` (src/main.rs)
//! because `custom_test_frameworks` harness generation (the thing that turns
//! `#[test_case]` functions into a callable `test_main()`) is a compiler
//! behaviour tied to `--test` compilation: it only happens when this file is
//! built via `cargo test` (which the Makefile does with `--no-run`, then
//! boots the resulting ELF itself under scripts/qemu.py -- see the
//! `[[bin]]` table in kernel/Cargo.toml and the GNUmakefile).
//!
//! Everything below is therefore split `cfg(test)` / `cfg(not(test))`: a
//! plain `cargo build`/`cargo clippy` (no `--test`) compiles this file too
//! (Cargo builds every bin target regardless of its `test =` field), and in
//! that mode `test_main()` doesn't exist and the `#[test_case]` functions
//! would just be dead code. The `cfg(not(test))` fallback keeps that build
//! clean (and inert: this binary is never booted outside test mode).
//!
//! The `#[test_case]` functions themselves live under `src/test_cases/`
//! (one file per subsystem, brief M1-T3 -- this file was nearing the ~600
//! line guideline once the heap tests joined the original GDT/IDT/PMM
//! ones), declared below as `mod test_cases;`. `custom_test_frameworks`
//! collects `#[test_case]` items crate-wide regardless of nesting, so
//! splitting them into submodules doesn't change how `test_main()` finds
//! them.
#![no_std]
#![no_main]
#![cfg_attr(test, feature(custom_test_frameworks))]
#![cfg_attr(test, test_runner(otteros_kernel::tests::test_runner))]
#![cfg_attr(test, reexport_test_harness_main = "test_main")]

// This bin target is its own crate, separate from the `otteros_kernel` lib
// (see the module docs above) -- so, like lib.rs, it needs its own
// `extern crate alloc;` for `test_cases::heap`'s `#[test_case]`s to use
// `alloc::{vec, string, boxed, collections}` directly (brief M1-T3). The
// one and only `#[global_allocator]` is still `otteros_kernel::mm::heap`'s;
// declaring `alloc` here doesn't create a second one.
#[cfg(test)]
extern crate alloc;

use otteros_kernel::{hlt_loop, init};
#[cfg(test)]
use otteros_kernel::cmdline;

/// `test pagefault` (briefs/M1-T1.md step 7): read one byte from an address
/// that is deliberately unmapped -- far above installed RAM, outside the
/// higher-half direct map, and outside the kernel image -- so this always
/// takes a page fault with a known CR2. `fault-test` (GNUmakefile) checks
/// serial for that exact address.
#[cfg(test)]
fn trigger_pagefault() {
    // Not written with `_` separators (`0xdead_beef_000`): clippy's
    // `unusual_byte_groupings` dislikes that grouping, and the printed
    // value (what `fault-test` actually greps for) doesn't have
    // underscores either way.
    let ptr = 0xdeadbeef000u64 as *const u8;
    // SAFETY: this address is unmapped by construction; the resulting page
    // fault is exactly what this test mode exercises, and `trap::trap_dispatch`
    // handles it (prints, then panics) without ever returning here.
    unsafe { core::ptr::read_volatile(ptr) };
}

/// `test doublefault` (briefs/M1-T1.md step 7): corrupt RSP to an unmapped
/// address, then `int3`. The CPU can't push the exception frame for the
/// breakpoint (RSP isn't valid) -> #PF; it can't push *that* frame either
/// -> #DF, delivered on IST1 regardless of the broken RSP. `df-test`
/// (GNUmakefile) checks serial for `DOUBLE FAULT`.
#[cfg(test)]
fn trigger_doublefault() -> ! {
    // SAFETY: deliberately corrupts RSP so this task's own stack is gone;
    // nothing here relies on it again; `noreturn` is accurate because
    // execution can never fall through to a `ret`/next-statement with no
    // valid stack -- the CPU's fault delivery (on IST1, independent of the
    // broken RSP) is the only way control leaves this asm block.
    unsafe {
        core::arch::asm!("mov rsp, 0xdead0000000", "int3", options(noreturn));
    }
}

/// `test stackoverflow` (brief M1-T4 step 5): recurse, touching a 1 KiB
/// local each level, until the guard page below the kernel's own
/// guard-paged stack (`mm::kstack`) is hit. `#PF` has no IST (see
/// `arch::x86_64::idt::ist_for`), so once a write lands in the unmapped
/// guard page, the CPU's own attempt to push that fault's interrupt frame
/// -- at the same, now-invalid `rsp` -- faults again, which
/// `trap::trap_dispatch`'s vector-8 arm reports as a stack overflow.
/// `#[inline(never)]` so this genuinely recurses instead of being folded
/// into a loop; the buffer is written byte-by-byte (not just declared) so
/// the compiler can't elide the stack allocation, and the recursive call
/// is used afterwards so it can't become a tail call either. `1 KiB` is
/// small enough that no single call can "jump over" the 4 KiB guard page
/// in one `sub rsp` (kernel-review would flag a larger frame for exactly
/// that risk). `stackoverflow-test` (GNUmakefile) checks serial for
/// `kernel stack overflow`.
#[cfg(test)]
#[inline(never)]
fn recurse_until_guard(depth: u64) -> u64 {
    let mut buf = [0u8; 1024];
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (depth ^ i as u64) as u8;
    }
    let next = recurse_until_guard(depth + 1);
    next.wrapping_add(u64::from(buf[(depth % 1024) as usize]))
}

#[cfg(test)]
fn trigger_stack_overflow() -> ! {
    recurse_until_guard(0);
    unreachable!("recursion should have hit the guard page (and double-faulted) before returning");
}

/// `test thread-stackoverflow` (brief M2-T1 step 10): the exact same
/// unbounded recursion as `trigger_stack_overflow` above, but run inside
/// a *spawned kernel thread*'s own stack (`sched::spawn`, via
/// `mm::kstack::allocate`) rather than the boot stack -- proves guard
/// pages work for a per-thread stack too, not just the one boot-time
/// stack every earlier M1 test exercised. `yield_now` hands the CPU to
/// it immediately (deterministic, rather than waiting on the next
/// preemption tick); the resulting double fault halts the kernel (via
/// `trap::trap_dispatch`'s vector-8 arm and `qemu::exit`) long before
/// anything here would run again. `thread-stackoverflow-test`
/// (GNUmakefile) checks serial for `kernel stack overflow`.
///
/// Brief M2-T3: `mm::kstack` now recycles a small, fixed set of slots
/// through a free list rather than bump-allocating a fresh one per
/// spawn -- the churn loop below spawns and joins a few hundred harmless
/// threads first, so the thread that actually overflows almost certainly
/// lands on a *reused* (non-zero-index) slot. That proves the
/// double-fault handler's own arithmetic guard check (`trap::
/// trap_dispatch`'s vector-8 arm, via `kstack::find_guard`) is correct for
/// a recycled slot, not merely the first one ever handed out.
#[cfg(test)]
fn trigger_thread_stack_overflow() -> ! {
    use otteros_kernel::sched;

    fn overflow_on_thread(_: usize) -> i32 {
        recurse_until_guard(0);
        unreachable!("recursion should have hit the spawned thread's own guard page first")
    }
    fn harmless(_: usize) -> i32 {
        0
    }

    for _ in 0..300 {
        let id = sched::spawn("test-stackoverflow-churn", harmless, 0);
        assert_eq!(sched::join(id), 0);
    }

    sched::spawn("test-thread-stackoverflow", overflow_on_thread, 0);
    sched::yield_now();
    unreachable!("the spawned thread's stack overflow should have double-faulted before this runs again")
}

/// `test pmm-double-free` (kernel-review, M1-T2 fix #3): allocate a frame,
/// free it, then free the exact same address again. `pmm-double-free-test`
/// (GNUmakefile) checks serial for `double free`.
#[cfg(test)]
fn trigger_pmm_double_free() {
    use otteros_kernel::mm::pmm;

    let phys = pmm::alloc_frame().expect("should have a free frame");
    pmm::free_frame(phys);
    pmm::free_frame(phys); // panics: "double free"
}

/// `test pmm-free-reserved` (kernel-review, M1-T2 fix #3): free an address
/// one frame past the highest USABLE address Limine reported. By
/// construction that's never inside any USABLE region -- it's either an
/// unlisted gap or (as in QEMU's map) the start of the next reserved/
/// bootloader-reclaimable entry -- so this exercises the ownership check
/// a real reserved region (or the kernel image) would hit, without
/// hardcoding a memory-map address that could shift between QEMU
/// versions. `pmm-free-reserved-test` (GNUmakefile) checks serial for
/// `not a usable frame`.
#[cfg(test)]
fn trigger_pmm_free_reserved() {
    use otteros_kernel::mm::addr::{FRAME_SIZE, PhysAddr};
    use otteros_kernel::mm::pmm;

    let one_past_usable = PhysAddr::new(pmm::stats().total as u64 * FRAME_SIZE as u64);
    pmm::free_frame(one_past_usable); // panics: "not a usable frame"
}

/// `test heap-double-free` (kernel-review, M1-T3 fix #1): allocate through
/// the heap, then `dealloc` the exact same pointer twice. The slab
/// allocator's per-slot `allocated` bitmap must catch the second call
/// before it corrupts the free list. `heap-double-free-test` (GNUmakefile)
/// checks serial for `heap: double free`.
#[cfg(test)]
fn trigger_heap_double_free() {
    use core::alloc::Layout;

    let layout = Layout::from_size_align(64, 8).expect("64/8 is a valid layout");
    // SAFETY: `layout` has non-zero size; the pointer is deliberately freed
    // twice below (that's what this test mode exercises), and nothing else
    // in this single-threaded test touches it in between.
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    assert!(!ptr.is_null(), "should have room for a 64-byte allocation this early in boot");
    // SAFETY: `ptr` was just allocated with exactly `layout` above.
    unsafe { alloc::alloc::dealloc(ptr, layout) };
    // SAFETY: deliberately a second free of the same, already-freed
    // pointer -- expected to panic ("heap: double free") rather than
    // corrupt the slab's free list.
    unsafe { alloc::alloc::dealloc(ptr, layout) };
}

/// `test heap-bad-class` (kernel-review, M1-T3 fix #1): allocate with a
/// 64-byte layout, then `dealloc` the same pointer with a 512-byte layout
/// -- a caller bug the slab allocator must catch rather than silently
/// running the wrong size class's free-list logic on it.
/// `heap-bad-class-test` (GNUmakefile) checks serial for
/// `heap: class mismatch`.
#[cfg(test)]
fn trigger_heap_bad_class() {
    use core::alloc::Layout;

    let alloc_layout = Layout::from_size_align(64, 8).expect("64/8 is a valid layout");
    let free_layout = Layout::from_size_align(512, 8).expect("512/8 is a valid layout");
    // SAFETY: `alloc_layout` has non-zero size; `ptr` is freed (with a
    // deliberately mismatched layout) below before this test ends.
    let ptr = unsafe { alloc::alloc::alloc(alloc_layout) };
    assert!(!ptr.is_null(), "should have room for a 64-byte allocation this early in boot");
    // SAFETY: deliberately frees `ptr` with a layout from a different size
    // class than the one it was allocated with -- expected to panic
    // ("heap: class mismatch") before touching any free list.
    unsafe { alloc::alloc::dealloc(ptr, free_layout) };
}

#[cfg(test)]
mod test_cases;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: the very first thing this kernel does, before relying on any
    // assumption about the interrupt flag Limine/firmware left us with
    // (kernel-review, M1-T1); disabling interrupts is always valid from
    // ring 0.
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };

    init(after_vmm)
}

/// Runs on the kernel's own guard-paged stack, after `init()` has built
/// the kernel's page tables, switched CR3 and moved off the stack Limine
/// handed it at entry (brief M1-T4) -- the rest of the M0/M1 test-mode
/// dispatch, unchanged from before that switch existed.
extern "C" fn after_vmm() -> ! {
    otteros_kernel::start_interrupts();

    #[cfg(test)]
    {
        let cmd = cmdline::get();
        if cmd.contains("panic") {
            panic!("deliberate M0-T1 test panic");
        } else if cmd.contains("pagefault") {
            trigger_pagefault();
        } else if cmd.contains("doublefault") {
            trigger_doublefault();
        } else if cmd.contains("thread-stackoverflow") {
            // Checked *before* the plain "stackoverflow" branch below:
            // "thread-stackoverflow" contains "stackoverflow" as a
            // substring, so the generic branch would otherwise always
            // shadow this one.
            trigger_thread_stack_overflow();
        } else if cmd.contains("stackoverflow") {
            trigger_stack_overflow();
        } else if cmd.contains("pmm-double-free") {
            trigger_pmm_double_free();
        } else if cmd.contains("pmm-free-reserved") {
            trigger_pmm_free_reserved();
        } else if cmd.contains("heap-double-free") {
            trigger_heap_double_free();
        } else if cmd.contains("heap-bad-class") {
            trigger_heap_bad_class();
        } else {
            test_main();
        }
    }

    // Only reached in a `cfg(not(test))` build, or if one of the branches
    // above somehow returned instead of exiting QEMU itself.
    hlt_loop();
}
