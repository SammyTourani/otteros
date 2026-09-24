//! The keyboard consumer (brief M1-T6 step 5): decodes the raw scancode
//! bytes `drivers::ps2`'s IRQ1 handler pushes into `RING`, and exposes
//! `poll_event`/`read_char_blocking` plus IRQ/drop counters to the rest
//! of the kernel.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::drivers::ps2::i8042;
use crate::drivers::ps2::ring::SpscRing;
use crate::drivers::ps2::scancode::{Decoder, Key, KeyEvent};
use crate::sched::WaitQueue;
use crate::sync::IrqMutex;

/// Raw scancode bytes between the IRQ1 handler and `poll_event`. Plenty
/// for bursts of key repeat or fast typing between two `poll_event`
/// calls; a full ring only drops (and counts) the newest byte, it never
/// blocks the interrupt handler.
const RING_CAPACITY: usize = 64;

static RING: SpscRing<u8, RING_CAPACITY> = SpscRing::new();
static DECODER: IrqMutex<Decoder> = IrqMutex::new(Decoder::new());
static IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

/// What `read_char_blocking` parks on instead of polling (brief M2-T1).
/// Woken from the IRQ1 handler (`push_scancode`), which -- per the
/// brief's design cautions ("wake-ups from IRQ context only enqueue") --
/// only ever moves a waiting thread back onto the ready queue; decoding
/// still happens only in normal context, in `poll_event`, exactly as
/// before this task.
static READ_QUEUE: WaitQueue = WaitQueue::new();

/// Whether a keyboard is actually usable -- `i8042::is_present`, exposed
/// here so callers only need to depend on `keyboard`, not `i8042`
/// directly.
pub fn is_available() -> bool {
    i8042::is_present()
}

/// Called only from `drivers::ps2`'s IRQ1 handler: pushes one raw
/// scancode byte and counts the interrupt. Never decodes here -- decoding
/// happens in `poll_event`, normal (non-IRQ) context, per the brief.
pub(crate) fn push_scancode(byte: u8) {
    IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    RING.push(byte);
    // Safe from IRQ context (brief M2-T1): `wake_one` only ever moves a
    // parked reader from `READ_QUEUE`'s own waiters list onto the
    // scheduler's ready queue, never allocates. A wake-up here doesn't
    // guarantee the pushed byte alone completes a character (it might be
    // a bare modifier/prefix byte) -- `read_char_blocking` re-checks and
    // loops if so, same as any other spurious wake-up.
    READ_QUEUE.wake_one();
}

/// How many keyboard IRQs have fired since boot.
pub fn irq_count() -> u64 {
    IRQ_COUNT.load(Ordering::Relaxed)
}

/// How many raw scancode bytes were dropped because the ring was full
/// (nothing called `poll_event` for a while).
pub fn dropped_count() -> u64 {
    RING.dropped()
}

/// Decodes and returns the next available key event, or `None` if the
/// ring has no more raw bytes right now. An `0xE0` prefix byte (or a
/// swallowed device ACK/Resend, see `scancode::Decoder::feed`) doesn't
/// complete an event by itself, so this keeps draining the ring until a
/// full event completes or it runs dry -- never blocks.
///
/// kernel-review, M1-T6 fix 1: `DECODER.lock()`'s guard must not still be
/// alive when `i8042::set_leds` runs below. `IrqMutex::lock` disables
/// interrupts for the guard's lifetime (`sync::IrqMutex`'s own docs), and
/// `set_leds` can wait (bounded, `i8042::wait_for`) on a `hlt` that only
/// the timer interrupt wakes -- a guard still held across that call would
/// disable the very interrupt the wait depends on, hanging forever
/// instead of timing out. Binding the decoded value first (`let decoded
/// = ...;`) drops the temporary guard at the end of that statement,
/// before `set_leds` (or anything else below) ever runs. Written as
/// `if let Some(event) = DECODER.lock().feed(byte) { ... }` instead, the
/// guard temporary would stay alive for the *entire* `if let` body (the
/// same temporary-lifetime-extension rule as `match`), keeping interrupts
/// disabled across `set_leds` too.
pub fn poll_event() -> Option<KeyEvent> {
    loop {
        let byte = RING.pop()?;
        let decoded = DECODER.lock().feed(byte);
        if let Some(event) = decoded {
            if event.key == Key::CapsLock && event.pressed {
                i8042::set_leds(event.mods.caps_lock);
            }
            return Some(event);
        }
    }
}

/// Converts a KeyEvent to its terminal byte representation (brief M2-T4b
/// step 1): special keys become ANSI escape sequences (e.g., Up -> ESC[A),
/// Ctrl+letter becomes 0x01..0x1A, Backspace becomes 0x7F, regular
/// characters are returned as-is.
pub fn key_event_to_bytes(event: &KeyEvent) -> alloc::vec::Vec<u8> {
    use alloc::vec;

    if !event.pressed {
        return vec![];
    }

    // Ctrl+letter: 0x01..0x1A
    if event.mods.ctrl {
        let ctrl_char = match event.key {
            Key::A => Some(0x01),
            Key::B => Some(0x02),
            Key::C => Some(0x03),
            Key::D => Some(0x04),
            Key::E => Some(0x05),
            Key::F => Some(0x06),
            Key::G => Some(0x07),
            Key::H => Some(0x08),
            Key::I => Some(0x09),
            Key::J => Some(0x0A),
            Key::K => Some(0x0B),
            Key::L => Some(0x0C),
            Key::M => Some(0x0D),
            Key::N => Some(0x0E),
            Key::O => Some(0x0F),
            Key::P => Some(0x10),
            Key::Q => Some(0x11),
            Key::R => Some(0x12),
            Key::S => Some(0x13),
            Key::T => Some(0x14),
            Key::U => Some(0x15),
            Key::V => Some(0x16),
            Key::W => Some(0x17),
            Key::X => Some(0x18),
            Key::Y => Some(0x19),
            Key::Z => Some(0x1A),
            _ => None,
        };
        if let Some(byte) = ctrl_char {
            return vec![byte];
        }
    }

    // Special keys with escape sequences
    match event.key {
        Key::Up => vec![0x1B, b'[', b'A'],
        Key::Down => vec![0x1B, b'[', b'B'],
        Key::Right => vec![0x1B, b'[', b'C'],
        Key::Left => vec![0x1B, b'[', b'D'],
        Key::Home => vec![0x1B, b'[', b'H'],
        Key::End => vec![0x1B, b'[', b'F'],
        Key::Delete => vec![0x1B, b'[', b'3', b'~'],
        Key::Backspace => vec![0x7F],
        _ => {
            // Regular character
            if let Some(ch) = event.to_char() {
                vec![ch as u8]
            } else {
                vec![]
            }
        }
    }
}

/// Blocks the calling thread (brief M2-T1: parked on `READ_QUEUE`, not
/// polling) until a full key event decodes to a character, and returns
/// it -- ignores break events and keys with no character (arrows,
/// modifiers, locks, ...).
pub fn read_char_blocking() -> char {
    loop {
        if let Some(event) = poll_event() {
            if let Some(ch) = event.to_char() {
                return ch;
            }
            continue;
        }
        READ_QUEUE.wait_until(|| !RING.is_empty());
    }
}
