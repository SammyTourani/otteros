//! PS/2 keyboard support (brief M1-T6, DECISIONS.md D9): the i8042
//! controller (`i8042`), scancode decoding (`scancode`), the lock-free
//! IRQ-to-consumer ring (`ring`), and the small consumer API `keyboard`
//! exposes to the rest of the kernel. `init` (called once from
//! `start_interrupts`) ties them together against real ACPI/IOAPIC data.

pub mod i8042;
pub mod keyboard;
pub mod ring;
pub mod scancode;

use crate::acpi;
use crate::acpi::madt::{Polarity as MadtPolarity, Trigger as MadtTrigger};
use crate::arch::x86_64::ioapic::{Polarity, Trigger};
use crate::arch::x86_64::trap::TrapFrame;
use crate::arch::x86_64::{ioapic, irq, lapic};
use crate::kprintln;

/// The vector IRQ1 (the keyboard) is routed to: the next one free after
/// the timer's (32, `time::TIMER_VECTOR`) and well below the LAPIC's
/// fixed hardware spurious vector (0xFF).
pub const KEYBOARD_VECTOR: u8 = 33;

/// Whether ACPI says a legacy i8042 exists on this machine (brief M1-T6):
/// the FADT's IA-PC Boot Architecture Flags bit 1, on any FADT that has
/// that field at all (revision >= 2). No FADT, an ACPI 1.0 FADT (the
/// field doesn't exist), or a too-short one all mean "assume one exists"
/// -- matching every real ACPI 1.0-era machine, which always has one.
fn acpi_reports_i8042() -> bool {
    acpi::with_info(|info| match info.iapc_boot_arch_flags {
        Some(flags) => flags & acpi::fadt::IAPC_BOOT_ARCH_8042 != 0,
        None => true,
    })
}

/// Resolves ISA IRQ `irq_num`'s polarity/trigger the same way
/// `arch::x86_64::irq::isa_to_gsi` resolves its GSI: the MADT's
/// Interrupt Source Override entry for it if one exists, else the ISA
/// bus defaults (`Polarity`/`Trigger::ConformsToBus`, resolved via
/// `resolve_for_isa`) -- converted from `acpi::madt`'s 3-variant
/// (`ConformsToBus` included) types to `ioapic`'s 2-variant ones, which
/// `resolve_for_isa` guarantees never sees `ConformsToBus` reach it.
fn isa_irq_polarity_trigger(irq_num: u8) -> (Polarity, Trigger) {
    let (polarity, trigger) = acpi::with_info(|info| {
        info.madt
            .overrides
            .iter()
            .find(|o| o.isa_irq == irq_num)
            .map(|o| (o.polarity, o.trigger))
            .unwrap_or((MadtPolarity::ConformsToBus, MadtTrigger::ConformsToBus))
    });

    let polarity = match polarity.resolve_for_isa() {
        MadtPolarity::ActiveLow => Polarity::ActiveLow,
        _ => Polarity::ActiveHigh, // resolve_for_isa only ever returns ActiveHigh/ActiveLow.
    };
    let trigger = match trigger.resolve_for_isa() {
        MadtTrigger::Level => Trigger::Level,
        _ => Trigger::Edge, // resolve_for_isa only ever returns Edge/Level.
    };
    (polarity, trigger)
}

/// The IRQ1 handler: reads the one byte that caused it and hands it to
/// the ring. No decoding here (brief M1-T6 step 3) -- that happens later,
/// in normal context, via `keyboard::poll_event`.
fn irq_handler(_frame: &mut TrapFrame) {
    keyboard::push_scancode(i8042::read_scancode_byte());
}

/// Brings up the keyboard end to end (brief M1-T6). Skips entirely (logs
/// `[kbd] no i8042 per ACPI`) if ACPI says no 8042 exists. Otherwise runs
/// `i8042::init`'s bring-up sequence, and -- only if that succeeded --
/// registers the IRQ1 handler *before* unmasking anything (so a stray
/// interrupt can never find no handler registered), then programs and
/// unmasks IRQ1's I/O APIC redirection entry.
///
/// Must run after `arch::x86_64::{lapic,ioapic}::init` (needs a live
/// LAPIC id and I/O APIC to program) and after `time::init` and
/// `arch::x86_64::irq::enable` (the i8042's own bounded waits need
/// `time::ticks()` actually advancing, which only happens once
/// interrupts are enabled for good) -- see `start_interrupts`.
pub fn init() {
    if !acpi_reports_i8042() {
        kprintln!("[kbd] no i8042 per ACPI");
        return;
    }

    i8042::init();
    if !i8042::is_present() {
        return;
    }

    irq::register(KEYBOARD_VECTOR, irq_handler);

    let gsi = irq::isa_to_gsi(1);
    let (polarity, trigger) = isa_irq_polarity_trigger(1);
    ioapic::set_redirect(gsi, KEYBOARD_VECTOR, lapic::id(), polarity, trigger, false);

    kprintln!("[kbd] IRQ1 -> gsi {gsi} -> vector {KEYBOARD_VECTOR}");
}
