//! PS/2 keyboard tests (brief M1-T6): the lock-free ring in isolation,
//! the scancode Set 1 decoder against known byte sequences, the i8042/IRQ
//! outcomes `drivers::ps2::init` already produced at boot (from
//! `start_interrupts`, before any test runs), and the QMP end-to-end
//! typing test.

use otteros_kernel::arch::x86_64::{ioapic, irq};
use otteros_kernel::drivers::ps2::ring::SpscRing;
use otteros_kernel::drivers::ps2::scancode::{Decoder, Key};
use otteros_kernel::drivers::ps2::{i8042, keyboard, KEYBOARD_VECTOR};
use otteros_kernel::{kprintln, time};

// --- ring::SpscRing -----------------------------------------------------

/// Values come back out in exactly the order they went in.
#[test_case]
fn ring_push_pop_in_order() {
    static RING: SpscRing<u8, 8> = SpscRing::new();
    RING.push(1);
    RING.push(2);
    RING.push(3);
    assert_eq!(RING.pop(), Some(1));
    assert_eq!(RING.pop(), Some(2));
    assert_eq!(RING.pop(), Some(3));
    assert_eq!(RING.pop(), None, "the ring should be empty again after popping everything pushed");
}

/// Many push/pop pairs on a small ring wrap the internal `head`/`tail`
/// indices around the backing array several times over; every value
/// must still round-trip.
#[test_case]
fn ring_wraps_around_capacity() {
    static RING: SpscRing<u8, 4> = SpscRing::new(); // 3 usable slots
    for round in 0..10u8 {
        RING.push(round);
        assert_eq!(RING.pop(), Some(round), "round {round} should round-trip even after several index wraps");
    }
}

/// Filling a ring to capacity and pushing one more must drop the new
/// value (not overwrite an unread one) and count it; everything already
/// buffered must still come back out untouched.
#[test_case]
fn ring_overflow_drops_newest_and_counts() {
    static RING: SpscRing<u8, 4> = SpscRing::new(); // 3 usable slots
    let before = RING.dropped();

    RING.push(10);
    RING.push(11);
    RING.push(12); // fills all 3 usable slots
    RING.push(13); // dropped: no room left

    assert_eq!(RING.dropped(), before + 1, "pushing into a full ring should count exactly one drop");
    assert_eq!(RING.pop(), Some(10), "already-buffered values must be unaffected by the dropped push");
    assert_eq!(RING.pop(), Some(11));
    assert_eq!(RING.pop(), Some(12));
    assert_eq!(RING.pop(), None);
}

// --- scancode::Decoder ---------------------------------------------------

/// `A`'s make (0x1E) and break (0x9E) codes decode to matching press/
/// release events for the same key.
#[test_case]
fn scancode_a_make_and_break() {
    let mut d = Decoder::new();
    let make = d.feed(0x1E).expect("0x1E is A's make code");
    assert_eq!(make.key, Key::A);
    assert!(make.pressed);

    let brk = d.feed(0x9E).expect("0x9E is A's break code");
    assert_eq!(brk.key, Key::A);
    assert!(!brk.pressed);
}

/// Holding shift while pressing `a` types an uppercase `A`.
#[test_case]
fn scancode_shift_a_is_uppercase() {
    let mut d = Decoder::new();
    d.feed(0x2A); // left shift, make
    let event = d.feed(0x1E).expect("0x1E is A's make code"); // 'a', make
    assert_eq!(event.to_char(), Some('A'));
}

/// Caps lock toggles on its own make code, stays latched through its own
/// break code, and toggles back off on a second make code.
#[test_case]
fn scancode_caps_lock_toggles_case() {
    let mut d = Decoder::new();

    let a_lower = d.feed(0x1E).expect("'a' make");
    assert_eq!(a_lower.to_char(), Some('a'));
    d.feed(0x9E); // 'a' break

    d.feed(0x3A); // caps lock make: toggles on
    d.feed(0xBA); // caps lock break: must NOT toggle again
    let a_upper = d.feed(0x1E).expect("'a' make");
    assert_eq!(a_upper.to_char(), Some('A'), "caps lock should still be latched on after its own break code");
    d.feed(0x9E);

    d.feed(0x3A); // toggles back off
    d.feed(0xBA);
    let a_lower_again = d.feed(0x1E).expect("'a' make");
    assert_eq!(a_lower_again.to_char(), Some('a'));
}

/// An `0xE0`-prefixed Right Arrow decodes to `Key::Right`, and the prefix
/// byte alone doesn't complete an event.
#[test_case]
fn scancode_e0_right_arrow_decodes() {
    let mut d = Decoder::new();
    assert_eq!(d.feed(0xE0), None, "a bare 0xE0 prefix shouldn't complete an event");
    let make = d.feed(0x4D).expect("0xE0 0x4D is the extended Right Arrow make code");
    assert_eq!(make.key, Key::Right);
    assert!(make.pressed);

    assert_eq!(d.feed(0xE0), None);
    let brk = d.feed(0xCD).expect("0xE0 0xCD is the extended Right Arrow break code");
    assert_eq!(brk.key, Key::Right);
    assert!(!brk.pressed);
}

/// The ctrl modifier is set for the make event and cleared again by the
/// matching break event.
#[test_case]
fn scancode_ctrl_modifier_set_while_held_and_cleared_on_break() {
    let mut d = Decoder::new();
    let make = d.feed(0x1D).expect("0x1D is left ctrl's make code");
    assert!(make.mods.ctrl, "ctrl should be set while the key is held");

    let brk = d.feed(0x9D).expect("0x9D is left ctrl's break code");
    assert!(!brk.mods.ctrl, "ctrl should be cleared once the key is released");
}

/// Shift plus a digit types that digit's shifted symbol (`1` -> `!`), not
/// the bare digit.
#[test_case]
fn scancode_shift_digit_gives_symbol() {
    let mut d = Decoder::new();
    d.feed(0x2A); // left shift, make
    let one = d.feed(0x02).expect("0x02 is '1'`s make code");
    assert_eq!(one.to_char(), Some('!'));
}

/// A device ACK (0xFA) byte -- as `i8042::set_leds` can trigger once IRQ1
/// is live -- is swallowed rather than misdecoded as some key's code.
#[test_case]
fn scancode_device_ack_is_swallowed() {
    let mut d = Decoder::new();
    assert_eq!(d.feed(0xFA), None, "0xFA (device ACK) should never decode to a key event");
    // The decoder must also still work normally afterward.
    let event = d.feed(0x1E).expect("0x1E is A's make code");
    assert_eq!(event.key, Key::A);
}

// --- drivers::ps2::i8042 / IRQ routing (live boot state) -----------------

/// `drivers::ps2::init` (called from `start_interrupts`, before any test
/// runs) already ran the whole i8042 bring-up sequence; on the QEMU q35
/// machine this suite targets, a real i8042 is always present, so this
/// must have succeeded.
#[test_case]
fn i8042_self_test_reported_present() {
    assert!(i8042::is_present(), "expected the i8042 bring-up sequence to have succeeded under QEMU q35");
}

/// Once `drivers::ps2::init` succeeds, IRQ1's Global System Interrupt
/// must be routed to `KEYBOARD_VECTOR` (33) and left unmasked.
#[test_case]
fn keyboard_gsi_redirect_unmasked_with_vector_33() {
    if !i8042::is_present() {
        kprintln!("[kbd] skipping keyboard_gsi_redirect_unmasked_with_vector_33: no i8042 present");
        return;
    }

    let gsi = irq::isa_to_gsi(1);
    let (low, _high) = ioapic::redirect_entry(gsi);
    assert_eq!(ioapic::entry_vector(low), KEYBOARD_VECTOR, "IRQ1's GSI should be routed to the keyboard vector");
    assert!(!ioapic::entry_is_masked(low), "IRQ1's redirect entry should be unmasked once init succeeds");
}

// --- end to end -----------------------------------------------------------

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
/// execution-order-wise, `custom_test_frameworks` runs a crate's tests
/// sorted by name -- immediately after this one) to consume.
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
