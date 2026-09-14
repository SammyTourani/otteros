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
#![no_std]
#![no_main]
#![cfg_attr(test, feature(custom_test_frameworks))]
#![cfg_attr(test, test_runner(otteros_kernel::tests::test_runner))]
#![cfg_attr(test, reexport_test_harness_main = "test_main")]

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

#[cfg(test)]
mod test_cases {
    use core::mem::{offset_of, size_of};

    use otteros_kernel::arch::x86_64::{gdt, idt, trap};
    use otteros_kernel::mm::addr::{FRAME_SIZE, PhysAddr, VirtAddr};
    use otteros_kernel::mm::bitmap::FrameBitmap;
    use otteros_kernel::mm::{hhdm, pmm};
    use otteros_kernel::{framebuffer, serial};

    /// Arithmetic is arithmetic, even in ring 0. `black_box` keeps the
    /// operands opaque to the optimiser so this is a genuine runtime check,
    /// not a compile-time-foldable tautology.
    #[test_case]
    fn trivial_arithmetic() {
        let a = core::hint::black_box(2_u64);
        let b = core::hint::black_box(2_u64);
        assert_eq!(a + b, 4);

        let x = core::hint::black_box(7_u64);
        let y = core::hint::black_box(6_u64);
        assert_eq!(x.wrapping_mul(y), 42);
    }

    /// Writes a byte out over COM1 and checks the UART actually reports it
    /// as sent (LSR transmit-holding-register-empty), not just that the
    /// call didn't crash.
    #[test_case]
    fn serial_write_roundtrip() {
        assert!(serial::self_test(), "UART never reported the byte as sent");
    }

    /// Writes a known pixel, tries a few wildly out-of-bounds writes, and
    /// confirms the known pixel is untouched -- i.e. bounds checking in
    /// `framebuffer::put_pixel` actually holds, not just "didn't panic".
    #[test_case]
    fn framebuffer_bounds() {
        assert!(
            framebuffer::bounds_self_test(),
            "an out-of-bounds framebuffer write corrupted memory, or there's no framebuffer"
        );
    }

    /// `gdt::init()` reloaded CS and SS from our own GDT, not Limine's.
    #[test_case]
    fn gdt_selectors_loaded() {
        let cs: u16;
        // SAFETY: reading a segment register into a GPR has no side effects.
        unsafe {
            core::arch::asm!("mov {0:x}, cs", out(reg) cs, options(nomem, nostack, preserves_flags));
        }
        assert_eq!(cs, gdt::KERNEL_CODE_SELECTOR);

        let ss: u16;
        // SAFETY: same as above, for SS.
        unsafe {
            core::arch::asm!("mov {0:x}, ss", out(reg) ss, options(nomem, nostack, preserves_flags));
        }
        assert_eq!(ss, gdt::KERNEL_DATA_SELECTOR);
    }

    /// `ltr` was actually executed: the Task Register holds our TSS selector.
    #[test_case]
    fn tss_loaded() {
        let tr: u16;
        // SAFETY: `str` (Store Task Register) only reads CPU state.
        unsafe {
            core::arch::asm!("str {0:x}", out(reg) tr, options(nomem, nostack, preserves_flags));
        }
        assert_eq!(tr, gdt::TSS_SELECTOR);
    }

    /// IST1 is set, 16-byte aligned, and actually points into the static
    /// double-fault stack (not, say, a leftover zero or a bogus value).
    #[test_case]
    fn tss_ist1_valid() {
        let ist1 = gdt::ist1();
        assert_ne!(ist1, 0);
        assert_eq!(ist1 % 16, 0, "IST1 (0x{ist1:x}) isn't 16-byte aligned");

        let (base, top) = gdt::df_stack_bounds();
        assert_eq!(ist1, top, "IST1 should be the *top* of the double-fault stack");
        assert!(ist1 > base);
    }

    /// All 32 CPU-exception gates are present, vector 8 (#DF) uses IST1,
    /// and vector 3 (#BP) is DPL3 so user code can `int3` later.
    #[test_case]
    fn idt_exception_gates_present() {
        for vector in 0..32 {
            assert!(idt::gate_present(vector), "vector {vector} has no gate");
        }
        assert_eq!(idt::gate_ist(8), 1);
        assert_eq!(idt::gate_dpl(3), 3);
    }

    /// `int3` dispatches to `trap_dispatch`, which counts it and returns --
    /// execution resumes right here, exactly once further along.
    #[test_case]
    fn breakpoint_returns() {
        let before = trap::breakpoint_count();
        // SAFETY: vector 3 is DPL0-reachable interrupt-gate breakpoint;
        // the handler counts it and returns, so control resumes here.
        unsafe { core::arch::asm!("int3", options(nomem, nostack, preserves_flags)) };
        assert_eq!(trap::breakpoint_count(), before + 1);
    }

    /// Same as `breakpoint_returns`, twice in a row, to rule out an
    /// off-by-one or a handler that only works once.
    #[test_case]
    fn breakpoint_twice() {
        let before = trap::breakpoint_count();
        // SAFETY: see `breakpoint_returns`.
        unsafe {
            core::arch::asm!("int3", options(nomem, nostack, preserves_flags));
            core::arch::asm!("int3", options(nomem, nostack, preserves_flags));
        }
        assert_eq!(trap::breakpoint_count(), before + 2);
    }

    /// Pins down `TrapFrame`'s size and the offsets of `vector`/`rip` to
    /// the exact layout `interrupts::common_stub` builds on the stack (see
    /// that module's doc comment): a regression here means the dispatcher
    /// is reading the wrong bytes for *every* exception.
    #[test_case]
    fn trapframe_layout() {
        assert_eq!(size_of::<trap::TrapFrame>(), 176, "TrapFrame must be exactly 22 * 8 bytes");
        assert_eq!(offset_of!(trap::TrapFrame, vector), 15 * 8);
        assert_eq!(offset_of!(trap::TrapFrame, error_code), 16 * 8);
        assert_eq!(offset_of!(trap::TrapFrame, rip), 17 * 8);
    }

    /// `trapframe_layout` only pins down struct offsets; it wouldn't
    /// notice e.g. two GPRs swapped in `common_stub`'s push order (the
    /// struct's overall size and its `vector`/`rip` offsets would still be
    /// right, since those don't depend on the *relative* order of the
    /// individual GPR pushes). This proves the actual values, not just the
    /// shape: load distinct, mutually-recognisable canaries into rbx/r12-
    /// r15, `int3`, and check `trap::trap_dispatch` decoded each one into
    /// the field with the matching name.
    #[test_case]
    fn trapframe_canaries() {
        const CANARY_RBX: u64 = 0xb000_0000_0000_00b1;
        const CANARY_R12: u64 = 0xc000_0000_0000_00c2;
        const CANARY_R13: u64 = 0xd000_0000_0000_00d3;
        const CANARY_R14: u64 = 0xe000_0000_0000_00e4;
        const CANARY_R15: u64 = 0xf000_0000_0000_00f5;

        // SAFETY: loads known, mutually-distinct values into rbx and
        // r12-r15, then `int3`s. rbx is callee-saved and LLVM reserves it
        // internally, so it can't be an `out(...)` clobber operand like
        // the others -- instead this saves/restores it itself with an
        // explicit (balanced) `push`/`pop`, which is why `options` below
        // can't claim `nomem`/`nostack` for this block even though r12-r15
        // are plain register clobbers. The breakpoint handler
        // (`trap::trap_dispatch`) copies the frame's decoded values into
        // `trap::LAST_BREAKPOINT_REGS` before returning here.
        unsafe {
            core::arch::asm!(
                "push rbx",
                "mov rbx, {rbx}",
                "mov r12, {r12}",
                "mov r13, {r13}",
                "mov r14, {r14}",
                "mov r15, {r15}",
                "int3",
                "pop rbx",
                rbx = const CANARY_RBX,
                r12 = const CANARY_R12,
                r13 = const CANARY_R13,
                r14 = const CANARY_R14,
                r15 = const CANARY_R15,
                out("r12") _,
                out("r13") _,
                out("r14") _,
                out("r15") _,
                options(preserves_flags),
            );
        }

        let regs = trap::last_breakpoint_regs();
        assert_eq!(regs.rbx, CANARY_RBX, "TrapFrame.rbx decoded the wrong value");
        assert_eq!(regs.r12, CANARY_R12, "TrapFrame.r12 decoded the wrong value");
        assert_eq!(regs.r13, CANARY_R13, "TrapFrame.r13 decoded the wrong value");
        assert_eq!(regs.r14, CANARY_R14, "TrapFrame.r14 decoded the wrong value");
        assert_eq!(regs.r15, CANARY_R15, "TrapFrame.r15 decoded the wrong value");
    }

    // --- mm::addr (brief M1-T2 step 1) ----------------------------------

    /// `align_down`/`align_up`/`is_aligned`/`frame_index` on both address
    /// newtypes -- exercised directly here since `pmm`'s own tests below
    /// only exercise them indirectly (via real allocations).
    #[test_case]
    fn addr_alignment_helpers() {
        let a = PhysAddr::new(0x1234);
        assert_eq!(a.align_down(0x1000).as_u64(), 0x1000);
        assert_eq!(a.align_up(0x1000).as_u64(), 0x2000);
        assert!(!a.is_aligned(0x1000));
        assert!(PhysAddr::new(0x2000).is_aligned(0x1000));
        assert_eq!(PhysAddr::new(0x3000).frame_index(), 3);

        let v = VirtAddr::new(0xffff_8000_0000_1234);
        assert_eq!(v.align_down(0x1000).as_u64(), 0xffff_8000_0000_1000);
        assert_eq!(v.align_up(0x1000).as_u64(), 0xffff_8000_0000_2000);
    }

    // --- mm::bitmap (brief M1-T2 step 3) ---------------------------------
    //
    // `FrameBitmap` is pure logic over caller-supplied storage, so these
    // exercise it directly over small on-stack arrays -- independent of
    // `pmm`'s real, RAM-sized bitmap tested below.

    #[test_case]
    fn bitmap_set_free_used_count() {
        let mut words = [0u64; 4]; // 256 frames
        let mut bmp = FrameBitmap::new(&mut words, 0, 256);
        bmp.fill_free();
        assert_eq!(bmp.count_free(), 256);

        bmp.set_used(10);
        assert!(bmp.is_used(10));
        assert_eq!(bmp.count_free(), 255);

        bmp.set_free(10);
        assert!(!bmp.is_used(10));
        assert_eq!(bmp.count_free(), 256);

        bmp.fill_used();
        assert_eq!(bmp.count_free(), 0);
        assert!(bmp.is_used(0));
        assert!(bmp.is_used(255));
    }

    #[test_case]
    fn bitmap_find_free_run_respects_alignment() {
        let mut words = [0u64; 2]; // 128 frames
        let mut bmp = FrameBitmap::new(&mut words, 0, 128);
        bmp.fill_used();
        for frame in 64..72 {
            bmp.set_free(frame);
        }

        // Exactly 8 free frames [64, 72); 8-frame-aligned start -> 64.
        assert_eq!(bmp.find_free_run(8, 8), Some(64));
        // Only 8 free frames total: asking for 9 must fail.
        assert_eq!(bmp.find_free_run(9, 1), None);
        // No start aligned to 5 has 8 consecutive free frames after it.
        assert_eq!(bmp.find_free_run(8, 5), None);
    }

    #[test_case]
    fn bitmap_find_free_run_none_when_full() {
        let mut words = [u64::MAX; 4]; // 256 frames, all used
        let bmp = FrameBitmap::new(&mut words, 0, 256);
        assert_eq!(bmp.find_free_run(1, 1), None);
    }

    // --- mm::pmm (brief M1-T2 step 4) ------------------------------------
    //
    // These share the one real, global PMM `mm::init()` already built from
    // the actual Limine memory map (see lib.rs's `init()`), so every test
    // below frees exactly what it allocates: leaving a frame permanently
    // allocated would shift every later test's view of "the lowest free
    // frame", not just its own.

    #[test_case]
    fn pmm_stats_sane() {
        let stats = pmm::stats();
        assert!(stats.total > 0, "pmm should have found some RAM");
        assert!(stats.free <= stats.total);
        assert_eq!(stats.used + stats.free, stats.total);
    }

    #[test_case]
    fn pmm_alloc_frame_valid() {
        let phys = pmm::alloc_frame().expect("pmm should have free frames");
        assert!(phys.is_aligned(FRAME_SIZE as u64), "0x{:x} isn't 4 KiB aligned", phys.as_u64());

        let (bitmap_base, bitmap_bytes) = pmm::bitmap_region();
        let addr = phys.as_u64();
        assert!(
            addr < bitmap_base.as_u64() || addr >= bitmap_base.as_u64() + bitmap_bytes as u64,
            "allocated the bitmap's own frame at 0x{addr:x}"
        );

        let mut in_usable_region = false;
        pmm::for_each_usable_region(|base, len| {
            if addr >= base.as_u64() && addr + FRAME_SIZE as u64 <= base.as_u64() + len {
                in_usable_region = true;
            }
        });
        assert!(in_usable_region, "0x{addr:x} is not inside any USABLE region");

        pmm::free_frame(phys);
    }

    #[test_case]
    fn pmm_two_allocations_differ() {
        let a = pmm::alloc_frame().expect("first alloc should succeed");
        let b = pmm::alloc_frame().expect("second alloc should succeed");
        assert_ne!(a, b);
        pmm::free_frame(a);
        pmm::free_frame(b);
    }

    #[test_case]
    fn pmm_alloc_then_free_all_restores_count() {
        let before = pmm::stats().free;

        let mut frames = [PhysAddr::new(0); 64];
        for slot in frames.iter_mut() {
            *slot = pmm::alloc_frame().expect("should have 64 free frames available");
        }
        for &frame in &frames {
            pmm::free_frame(frame);
        }

        assert_eq!(pmm::stats().free, before);
    }

    /// Dirties a frame, frees it, then immediately calls
    /// `alloc_frame_zeroed`: nothing else runs on this single core between
    /// the free and the next alloc, and the allocator is a deterministic
    /// lowest-free-frame search, so it is guaranteed to hand back the
    /// exact same frame -- letting this test prove `alloc_frame_zeroed`
    /// actually zeroes it, rather than just trusting RAM happened to
    /// start zeroed.
    #[test_case]
    fn pmm_alloc_frame_zeroed_is_zero() {
        let dirty = pmm::alloc_frame().expect("should have a free frame");
        let dirty_virt = hhdm::phys_to_virt(dirty);
        // SAFETY: `dirty` was just allocated by `alloc_frame`, so this
        // test exclusively owns it for as long as it stays allocated; the
        // HHDM maps every physical frame the PMM covers 1:1, so
        // `dirty_virt` is valid and writable for `FRAME_SIZE` bytes.
        unsafe { core::ptr::write_bytes(dirty_virt.as_u64() as *mut u8, 0xAA, FRAME_SIZE) };
        pmm::free_frame(dirty);

        let zeroed = pmm::alloc_frame_zeroed().expect("should have a free frame");
        assert_eq!(zeroed, dirty, "allocator wasn't deterministic between the free and this alloc");

        let zeroed_virt = hhdm::phys_to_virt(zeroed);
        // SAFETY: `zeroed` was just allocated (and zeroed) by
        // `alloc_frame_zeroed` above; same reasoning as the write above,
        // this time reading the bytes back.
        let bytes =
            unsafe { core::slice::from_raw_parts(zeroed_virt.as_u64() as *const u8, FRAME_SIZE) };
        assert!(bytes.iter().all(|&b| b == 0), "alloc_frame_zeroed did not zero the frame");

        pmm::free_frame(zeroed);
    }

    #[test_case]
    fn pmm_alloc_contiguous_aligned() {
        let phys = pmm::alloc_contiguous(8, 8).expect("should find a free 8-frame run");
        assert_eq!(
            phys.as_u64() % (8 * FRAME_SIZE as u64),
            0,
            "0x{:x} isn't aligned to 32 KiB",
            phys.as_u64()
        );

        let base_frame = phys.frame_index();
        for i in 0..8u64 {
            let frame_addr = PhysAddr::new((base_frame + i) * FRAME_SIZE as u64);
            assert!(
                pmm::is_frame_used(frame_addr),
                "frame {} of the contiguous run isn't marked used",
                base_frame + i
            );
        }

        pmm::free_contiguous(phys, 8);
    }

    #[test_case]
    fn pmm_frame_zero_never_returned() {
        for _ in 0..1000 {
            let phys = pmm::alloc_frame().expect("should have free frames for 1000 cycles");
            assert_ne!(phys.as_u64(), 0, "frame 0 was handed out by alloc_frame");
            pmm::free_frame(phys);
        }
    }

    /// `find_free_run` over a free range that straddles a 64-bit word
    /// boundary (frames 60..70 span word 0's top and word 1's bottom,
    /// since each word covers 64 frames) -- exercises `first_used_rel`'s
    /// partial-word-on-both-sides path, not just the whole-word fast path
    /// `bitmap_find_free_run_respects_alignment` already covers.
    #[test_case]
    fn bitmap_find_free_run_straddles_word_boundary() {
        let mut words = [0u64; 2]; // 128 frames; word 0 = [0,64), word 1 = [64,128)
        let mut bmp = FrameBitmap::new(&mut words, 0, 128);
        bmp.fill_used();
        for frame in 60..70 {
            bmp.set_free(frame);
        }

        assert_eq!(bmp.find_free_run(8, 1), Some(60));
        assert_eq!(bmp.find_free_run(8, 4), Some(60)); // 60 is itself 4-aligned
    }

    /// `stats().low_reserved` (kernel-review, M1-T2 fix #2) should reflect
    /// the frames below 1 MiB that Limine called USABLE -- QEMU's default
    /// map always has some (conventional-memory `0x0-0x9fc00`-ish), and it
    /// can never exceed the 256 frames a 1 MiB / 4 KiB split allows for.
    #[test_case]
    fn pmm_stats_low_reserved_sane() {
        let stats = pmm::stats();
        assert!(stats.low_reserved > 0, "low memory reservation should never be zero on QEMU");
        assert!(stats.low_reserved <= 256, "can't reserve more than the 256 frames below 1 MiB");
    }

    #[test_case]
    fn pmm_alloc_frame_low_returns_low_address() {
        let phys = pmm::alloc_frame_low().expect("should have a free low-memory frame");
        assert!(phys.as_u64() < 0x10_0000, "0x{:x} is not below 1 MiB", phys.as_u64());
        assert!(phys.is_aligned(FRAME_SIZE as u64));
        assert_ne!(phys.as_u64(), 0, "frame 0 was handed out by alloc_frame_low");

        pmm::free_frame(phys);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: the very first thing this kernel does, before relying on any
    // assumption about the interrupt flag Limine/firmware left us with
    // (kernel-review, M1-T1); disabling interrupts is always valid from
    // ring 0.
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };

    init();

    #[cfg(test)]
    {
        let cmd = cmdline::get();
        if cmd.contains("panic") {
            panic!("deliberate M0-T1 test panic");
        } else if cmd.contains("pagefault") {
            trigger_pagefault();
        } else if cmd.contains("doublefault") {
            trigger_doublefault();
        } else if cmd.contains("pmm-double-free") {
            trigger_pmm_double_free();
        } else if cmd.contains("pmm-free-reserved") {
            trigger_pmm_free_reserved();
        } else {
            test_main();
        }
    }

    // Only reached in a `cfg(not(test))` build, or if one of the branches
    // above somehow returned instead of exiting QEMU itself.
    hlt_loop();
}
