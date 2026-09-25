//! Kernel monotonic time (DECISIONS.md D19): the LAPIC timer, calibrated
//! against the legacy PIT and then run in periodic mode at 1 kHz, is both
//! the scheduler tick and `uptime_ms`'s clock. TSC/RTC arrive later (D19)
//! once something needs finer calibration or wall-clock time.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::trap::TrapFrame;
use crate::arch::x86_64::{irq, lapic, pit};
use crate::kprintln;

/// The scheduler-tick vector (brief M1-T5 step 5): fixed at 32, the
/// first IRQ vector -- exactly where the legacy PIT's IRQ0 would have
/// landed had the legacy PIC still been in charge (brief step 3: it's
/// remapped there too, but fully masked, so there's no ambiguity about
/// which device vector 32 actually belongs to on this kernel).
pub const TIMER_VECTOR: u8 = 32;
/// The kernel's fixed scheduler-tick rate.
const TARGET_HZ: u64 = 1000;
/// How long the PIT-timed calibration window lasts.
const CALIBRATION_MS: u64 = 10;
/// Add a jitter sample to the CSPRNG every N timer ticks (brief M8-T6b).
const JITTER_SAMPLE_INTERVAL: u64 = 16;

static TICKS: AtomicU64 = AtomicU64::new(0);

/// The LAPIC (divide-by-16) ticks-per-millisecond `init` measured against
/// the PIT -- the same value it programs as the periodic reload. Recorded
/// so a test can independently confirm the *hardware* reload register
/// actually holds this (kernel-review, M1-T5 fix #4): the wide delivered-
/// rate tolerance `test_cases::timer` otherwise needs (this host's real
/// interrupt-delivery ceiling, not a calibration property) can't by
/// itself catch a calibration/divisor bug that happens to still land in
/// that wide range.
static CALIBRATED_TICKS_PER_MS: AtomicU64 = AtomicU64::new(0);

fn on_tick(_frame: &mut TrapFrame) {
    let tick_count = TICKS.fetch_add(1, Ordering::Relaxed);
    // Brief M2-T1: scheduler bookkeeping that must happen every tick --
    // waking due sleepers and counting down the current thread's
    // timeslice -- lives in `sched` itself, not here; this is just the
    // hook that drives it. Never allocates, never logs (`sched::
    // on_timer_tick`'s own docs).
    crate::sched::on_timer_tick();
    // Brief M8-T6b: gather TSC-jitter samples every 16 ticks for the CSPRNG,
    // using try_lock to never block in IRQ context.
    if (tick_count + 1).is_multiple_of(JITTER_SAMPLE_INTERVAL) {
        crate::random::add_jitter_sample();
    }
}

/// The ticks-per-millisecond `init` calibrated against the PIT, and then
/// used as the periodic reload value.
pub fn calibrated_ticks_per_ms() -> u64 {
    CALIBRATED_TICKS_PER_MS.load(Ordering::Relaxed)
}

/// Ticks (at `TARGET_HZ`, 1000 per second) since the timer started.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Milliseconds since the timer started -- identical to `ticks()` at
/// exactly 1000 Hz, kept as a separate name for callers that mean
/// "elapsed time" rather than "scheduler ticks" (the two happen to be the
/// same number today, DECISIONS.md D19).
pub fn uptime_ms() -> u64 {
    ticks()
}

/// Busy-waits, `hlt`ing between checks (brief M1-T5: every wait after
/// interrupts are enabled parks the CPU instead of spinning it), until at
/// least `ms` milliseconds of ticks have elapsed.
pub fn sleep_ms(ms: u64) {
    let target = ticks().saturating_add(ms);
    while ticks() < target {
        // SAFETY: `hlt` halts until the next interrupt (the timer tick
        // this loop is waiting on, if nothing else fires first) and is
        // always valid to execute from ring 0; `sleep_ms` is only ever
        // reachable after `init` below has already programmed and
        // unmasked the periodic timer, so this always wakes back up.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)) };
    }
}

/// Calibrates the LAPIC timer against the PIT (brief M1-T5 step 5) --
/// channel 2, one-shot, `CALIBRATION_MS` -- then programs it for
/// periodic, `TARGET_HZ` operation on `TIMER_VECTOR`, registers the tick
/// handler, and unmasks it. Must run after `arch::x86_64::lapic::init`
/// (needs a live LAPIC base) and before `arch::x86_64::irq::enable`
/// (`sti`) -- the very first real tick must always find `on_tick` already
/// registered.
pub fn init() {
    lapic::set_timer_divide_by_16();
    lapic::set_timer_lvt(TIMER_VECTOR, false, true); // one-shot, masked: this never actually raises an interrupt.

    lapic::set_timer_initial_count(u32::MAX);
    pit::wait_ms(CALIBRATION_MS as u32);
    let elapsed_ticks = u64::from(u32::MAX - lapic::timer_current_count());

    let ticks_per_ms = (elapsed_ticks / CALIBRATION_MS).max(1);
    CALIBRATED_TICKS_PER_MS.store(ticks_per_ms, Ordering::Relaxed);
    let reload = ticks_per_ms.min(u64::from(u32::MAX)) as u32;

    irq::register(TIMER_VECTOR, on_tick);

    lapic::set_timer_lvt(TIMER_VECTOR, true, false); // periodic, unmasked.
    lapic::set_timer_initial_count(reload);

    kprintln!("[timer] lapic timer calibrated: {elapsed_ticks}/{CALIBRATION_MS}ms, periodic {TARGET_HZ} Hz");
}
