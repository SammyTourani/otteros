//! The QMP keyboard end-to-end tests, split out of `test_cases::ps2`
//! (brief M2-T2b, suite robustness under host CPU contention) so they run
//! early in the suite: `custom_test_frameworks` runs a crate's tests
//! sorted by fully qualified name, which is a *global* sort over
//! `module::function`, not source declaration order within `test_cases::
//! mod`'s `mod` list (confirmed empirically -- reordering those `mod`
//! lines alone changes nothing) -- so the only way to move these two
//! tests ahead of, say, `test_cases::proc`'s or `test_cases::sched`'s is
//! to put them in a module whose own name sorts earlier. `keyboard_e2e` <
//! `payloads` < `proc` < `ps2` < `sched` does exactly that: `scripts/
//! qemu.py --send-keys`'s wait for `[kbd] ready` on serial now has to
//! outlast only the handful of fast, non-QEMU-interactive modules that
//! still sort before it (`acpi`, `console`, `heap`, `irq`), not also the
//! hundreds of process/thread spawn-and-join cycles `test_cases::proc`/
//! `test_cases::sched` run later in the suite.
//!
//! `test_cases::ps2` keeps every *other* PS/2 test (the lock-free ring,
//! the scancode decoder, the i8042/IRQ bring-up checks) -- none of those
//! wait on external QMP input, so there's no benefit to moving them too.

use otteros_kernel::drivers::ps2::{i8042, keyboard};
use otteros_kernel::{kprintln, time};

/// End to end (brief M1-T6 step 6): prints `[kbd] ready` -- the line
/// `scripts/qemu.py --send-keys` waits for on serial before injecting
/// anything over QMP -- then blocks, bounded by a 5 s `time::ticks()`
/// budget (never forever: a real keyboard, or a misconfigured/absent
/// `--send-keys`, must fail this one test rather than hang the whole
/// suite), until 5 characters have decoded, and checks they spell
/// `hello`. Skipped (not failed) if this machine genuinely has no i8042:
/// there's nothing it could possibly receive.
///
/// `GNUmakefile`'s `test`/`bios-test` targets inject more than just
/// `hello` (`--send-keys hello,caps_lock,h,caps_lock`): this test only
/// consumes the first 5 decoded characters and returns, deliberately
/// leaving the rest (`caps_lock,h,caps_lock`) sitting in the shared ring
/// for `keyboard_e2e_caps_lock_led` (alphabetically -- and so
/// execution-order-wise -- immediately after this one, the only other
/// item in this module) to consume.
#[test_case]
fn keyboard_e2e() {
    if !i8042::is_present() {
        kprintln!("[kbd] skipping keyboard_e2e: no i8042 present");
        return;
    }

    kprintln!("[kbd] ready");

    const EXPECTED: &str = "hello";
    const TIMEOUT_MS: u64 = 5000;
    let mut received = alloc::string::String::new();
    let deadline = time::ticks().saturating_add(TIMEOUT_MS);

    while received.len() < EXPECTED.len() {
        if let Some(ch) = keyboard::poll_event().and_then(|event| event.to_char()) {
            received.push(ch);
            continue;
        }
        assert!(time::ticks() < deadline, "keyboard_e2e: timed out after {TIMEOUT_MS}ms, got {received:?} so far");
        // SAFETY: `hlt` halts until the next interrupt (the injected
        // keystroke's own IRQ1, if it hasn't landed yet, or the periodic
        // timer otherwise) and is always valid to execute from ring 0;
        // interrupts have been enabled for good since `start_interrupts`,
        // long before any test runs.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)) };
    }

    assert_eq!(received, EXPECTED, "expected the QMP-injected keystrokes to decode to {EXPECTED:?}");
}

/// End to end (kernel-review, M1-T6 fix 6): exercises the Caps Lock LED
/// path with IRQ1 already live -- `keyboard::poll_event`'s call to
/// `i8042::set_leds` whenever the decoder toggles caps lock, and the
/// decoder's own shift/caps-lock case logic -- by reading the characters
/// `scripts/qemu.py --send-keys` injects right after `hello`
/// (`caps_lock,h,caps_lock`, continuing on from `keyboard_e2e` above) and
/// checking the first decoded character is the uppercase `H`. Bounded
/// the same way as `keyboard_e2e` (a 5 s `time::ticks()` budget, never
/// forever). This is exactly the scenario that would have hung the whole
/// suite before `keyboard::poll_event` stopped holding `DECODER`'s
/// `IrqMutex` guard across `i8042::set_leds`'s hardware wait
/// (kernel-review fix 1): a Caps Lock press deadlocking a `hlt` that only
/// the timer interrupt -- disabled for as long as that guard lived --
/// could ever have satisfied. Skipped (not failed) if this machine
/// genuinely has no i8042.
#[test_case]
fn keyboard_e2e_caps_lock_led() {
    if !i8042::is_present() {
        kprintln!("[kbd] skipping keyboard_e2e_caps_lock_led: no i8042 present");
        return;
    }

    const TIMEOUT_MS: u64 = 5000;
    let deadline = time::ticks().saturating_add(TIMEOUT_MS);

    let ch = loop {
        if let Some(ch) = keyboard::poll_event().and_then(|event| event.to_char()) {
            break ch;
        }
        assert!(
            time::ticks() < deadline,
            "keyboard_e2e_caps_lock_led: timed out after {TIMEOUT_MS}ms waiting for the post-caps-lock character"
        );
        // SAFETY: identical reasoning to `keyboard_e2e`'s own `hlt`.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)) };
    };

    assert_eq!(ch, 'H', "expected caps lock to be latched on (uppercase H) after the injected caps_lock,h sequence");
}
