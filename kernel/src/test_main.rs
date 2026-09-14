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

#[cfg(test)]
mod test_cases {
    use core::mem::{offset_of, size_of};

    use otteros_kernel::arch::x86_64::{gdt, idt, trap};
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
        } else {
            test_main();
        }
    }

    // Only reached in a `cfg(not(test))` build, or if one of the branches
    // above somehow returned instead of exiting QEMU itself.
    hlt_loop();
}
