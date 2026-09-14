//! The legacy 8253/8254 Programmable Interval Timer (brief M1-T5 step 5):
//! channel 2, used only as a one-shot, polled reference clock for
//! calibrating the LAPIC timer -- and, in tests, for independently timing
//! a window to measure the LAPIC timer's real rate against. Never wired
//! to an interrupt: channels 0/1 are untouched entirely (DECISIONS.md
//! D19 -- the LAPIC timer is the scheduler tick, not the PIT).

use crate::arch::x86_64::port::{inb, outb};

/// The PIT's fixed input clock frequency (Intel 8253/8254 datasheet;
/// every PC-compatible platform, QEMU included, wires this exactly).
const PIT_HZ: u64 = 1_193_182;

const CHANNEL2_DATA: u16 = 0x42;
const MODE_COMMAND: u16 = 0x43;
/// System Control Port A (the PS/2-era "port 61h"): bit 0 gates channel
/// 2's clock input, bit 1 routes its output to the speaker, bit 5 reads
/// that same output back regardless of the speaker-routing bit.
const CONTROL_PORT: u16 = 0x61;
const CONTROL_GATE: u8 = 1 << 0;
const CONTROL_SPEAKER: u8 = 1 << 1;
const CONTROL_OUT2_STATUS: u8 = 1 << 5;

/// Channel 2, lobyte/hibyte access, mode 0 (interrupt-on-terminal-count --
/// though nothing here ever unmasks the interrupt that mode is named
/// for; only the output pin, polled via `CONTROL_OUT2_STATUS`, matters):
/// binary `10 11 000 0`.
const MODE_CHANNEL2_ONE_SHOT: u8 = 0b1011_0000;

/// One one-shot wait, up to the largest window a single 16-bit reload
/// value can express at the PIT's fixed input frequency (~54.9 ms).
const MAX_SINGLE_WAIT_MS: u32 = 50;

fn one_shot_wait(ms: u32) {
    debug_assert!(ms > 0 && ms <= MAX_SINGLE_WAIT_MS, "pit::one_shot_wait: {ms} ms doesn't fit one 16-bit reload");
    let count = ((PIT_HZ * u64::from(ms)) / 1000) as u16;

    // SAFETY: ports 0x61/0x42/0x43 are the PC-compatible System Control
    // Port A and PIT channel-2 data/command ports, always present on
    // every PC-compatible platform this kernel targets (D1, QEMU's `q35`
    // machine included). Gating channel 2 on, silencing its speaker
    // route, and loading a one-shot countdown is exactly the documented
    // 8253/8254 protocol for this mode.
    unsafe {
        let control = inb(CONTROL_PORT);
        outb(CONTROL_PORT, (control & !CONTROL_SPEAKER) | CONTROL_GATE);
        outb(MODE_COMMAND, MODE_CHANNEL2_ONE_SHOT);
        outb(CHANNEL2_DATA, (count & 0xFF) as u8);
        outb(CHANNEL2_DATA, (count >> 8) as u8);
    }

    // SAFETY: polling a status bit has no side effects.
    while unsafe { inb(CONTROL_PORT) } & CONTROL_OUT2_STATUS == 0 {
        core::hint::spin_loop();
    }
}

/// Busy-waits (spinning -- this is the reference clock calibration itself
/// uses, before the LAPIC timer that would ever wake a `hlt` is
/// programmed, and tests use it the same way for an independent
/// measurement window) for approximately `ms` milliseconds, chaining
/// one-shot windows as needed for anything longer than one 16-bit reload
/// can express.
pub fn wait_ms(ms: u32) {
    let mut remaining = ms;
    while remaining > 0 {
        let chunk = remaining.min(MAX_SINGLE_WAIT_MS);
        one_shot_wait(chunk);
        remaining -= chunk;
    }
}
