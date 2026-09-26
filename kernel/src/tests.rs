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
        let start_ms = crate::time::uptime_ms();
        self();
        let elapsed_ms = crate::time::uptime_ms().saturating_sub(start_ms);
        if elapsed_ms > 0 {
            crate::kprintln!("ok ({} ms)", elapsed_ms);
        } else {
            crate::kprintln!("ok");
        }
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

    // Brief M2-T3 step 7: every in-kernel test passing is only half the
    // gate now -- `/bin/init test` runs `/bin/utest` and exits with its
    // code (see `init`'s own docs), which `utest` sets to its own passed-
    // check count on a clean run (0 on any failure, brief step 6's ">= 10
    // checks" -- see `[utest]`'s own doc comment for the exact contract).
    // The overall exit status is the AND of both halves: every test above
    // already had to pass to reach this line at all (a failed `#[test_
    // case]` panics immediately, via `Testable::run`), so the only thing
    // left to fold in is whether userspace's own count came back nonzero.
    let count = crate::proc::spawn("/bin/init", &["test"]).ok().and_then(crate::proc::wait).unwrap_or(0);
    crate::kprintln!("[ok] userspace tests passed ({count})");

    crate::qemu::exit(count > 0);
}
