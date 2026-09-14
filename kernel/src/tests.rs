//! In-kernel test runner glue for `custom_test_frameworks`.
//!
//! The `#[test_case]` functions themselves live in the `otteros-kernel-test`
//! binary (see `src/test_main.rs`), not here: Rust only collects
//! `#[test_case]` items that belong to the crate the `--test` harness is
//! generated for, and that crate is the `otteros-kernel-test` bin, not this
//! shared library.

/// Wraps a `#[test_case]` function so we can log its name (captured before
/// it's erased to `&dyn Testable`) either side of running it.
pub trait Testable {
    fn run(&self);
}

impl<T: Fn()> Testable for T {
    fn run(&self) {
        crate::kprint!("[test] {} ... ", core::any::type_name::<T>());
        self();
        crate::kprintln!("ok");
    }
}

/// The `#[test_runner]` for `otteros-kernel-test`: runs every collected
/// test, prints the `[ok] tests passed (N)` line the harness and humans
/// both look for, then exits QEMU with the isa-debug-exit success code.
/// Never returns.
///
/// The summary line is wrapped in `ESC[1;32m...ESC[0m` (bold green) --
/// deliberately, not just decoration: `console::feed` sees it on every
/// `gmake test`/`bios-test` run, so `serial::SerialPort`'s ANSI filter
/// (brief M1-T7 fix 2: colour reaches the console, never
/// `artifacts/serial.log`) is genuinely exercised every time, not just
/// when something happens to print a colourful banner.
pub fn test_runner(tests: &[&dyn Testable]) -> ! {
    crate::kprintln!("[test] running {} test(s)", tests.len());
    for test in tests {
        test.run();
    }
    crate::kprintln!("\u{1b}[1;32m[ok] tests passed ({})\u{1b}[0m", tests.len());
    crate::qemu::exit(true);
}
