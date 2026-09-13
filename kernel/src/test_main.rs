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

#[cfg(test)]
mod test_cases {
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
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    init();

    #[cfg(test)]
    {
        if cmdline::get().contains("panic") {
            panic!("deliberate M0-T1 test panic");
        }
        test_main();
    }

    // Only reached in a `cfg(not(test))` build, or if `test_main()` somehow
    // returned instead of exiting QEMU itself.
    hlt_loop();
}
