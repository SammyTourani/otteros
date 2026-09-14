//! ACPI table discovery tests (brief M1-T5): RSDP/SDT checksum arithmetic
//! (both directly, over synthetic bytes, and re-run live against the
//! real RSDP `acpi::init` already trusted at boot) and MADT/LAPIC
//! cross-checks against the machine QEMU actually describes.

use otteros_kernel::acpi::{self, madt, rsdp};
use otteros_kernel::arch::x86_64::lapic;

/// `acpi::checksum_is_zero` is the one piece of arithmetic every RSDP/SDT
/// validation in this module ultimately relies on -- exercised directly
/// over synthetic bytes (independent of whatever real ACPI tables QEMU
/// happens to provide) so a broken implementation can't hide behind
/// "the real tables happened to still parse".
#[test_case]
fn acpi_checksum_arithmetic() {
    let mut bytes = [1u8, 2, 3, 4, 0u8];
    let partial: u8 = bytes[..4].iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    bytes[4] = 0u8.wrapping_sub(partial);
    assert!(acpi::checksum_is_zero(&bytes), "a correctly chosen checksum byte should sum to zero");

    bytes[4] = bytes[4].wrapping_add(1);
    assert!(!acpi::checksum_is_zero(&bytes), "corrupting one byte should break the checksum");
}

/// Re-validates the exact RSDP `acpi::init` already trusted at boot (boot
/// would have halted on `[acpi] FATAL: RSDP checksum invalid` otherwise)
/// -- a live, hardware-derived positive test of `rsdp::Rsdp::parse`, not
/// just a re-statement of the pure-arithmetic test above.
#[test_case]
fn acpi_rsdp_checksum_valid() {
    let phys = acpi::with_info(|info| info.rsdp_phys);
    let parsed = rsdp::Rsdp::parse(phys);
    assert!(parsed.is_some(), "the RSDP boot already relied on should still validate");
}

/// `acpi::init` only ever counts a table in `sdt_count` after
/// `sdt::collect_headers` validated its checksum (`acpi::sdt::validate`) --
/// so `sdt_count >= 1` means at least one real SDT (in practice, at
/// least the MADT) had a valid checksum.
#[test_case]
fn acpi_at_least_one_sdt_validated() {
    let sdt_count = acpi::with_info(|info| info.sdt_count);
    assert!(sdt_count >= 1, "expected at least one checksummed SDT (e.g. the MADT itself)");
}

/// The MADT must describe at least one enabled CPU and at least one I/O
/// APIC for this kernel to have anything to program.
#[test_case]
fn acpi_madt_has_cpu_and_ioapic() {
    acpi::with_info(|info| {
        let enabled_cpus = info.madt.cpus.iter().filter(|c| c.enabled).count();
        assert!(enabled_cpus >= 1, "MADT should list at least one enabled CPU");
        assert!(!info.madt.ioapics.is_empty(), "MADT should list at least one I/O APIC");
    });
}

/// Table-driven (kernel-review, M1-T5 fix #7): all four 2-bit MPS INTI
/// polarity encodings and all four trigger encodings, each checked
/// against its documented (ACPI 6.x table 5-38) meaning -- not just the
/// two encodings a real MADT happens to use, which would never notice a
/// mixed-up `_ =>` fallback arm for the "reserved" ones.
#[test_case]
fn madt_polarity_and_trigger_flag_decoding() {
    const POLARITY_CASES: [(u16, madt::Polarity); 4] = [
        (0b00, madt::Polarity::ConformsToBus),
        (0b01, madt::Polarity::ActiveHigh),
        (0b10, madt::Polarity::ConformsToBus), // reserved encoding
        (0b11, madt::Polarity::ActiveLow),
    ];
    for &(flags, expected) in &POLARITY_CASES {
        assert_eq!(madt::polarity_from_flags(flags), expected, "polarity_from_flags({flags:#04b})");
    }

    const TRIGGER_CASES: [(u16, madt::Trigger); 4] = [
        (0b00 << 2, madt::Trigger::ConformsToBus),
        (0b01 << 2, madt::Trigger::Edge),
        (0b10 << 2, madt::Trigger::ConformsToBus), // reserved encoding
        (0b11 << 2, madt::Trigger::Level),
    ];
    for &(flags, expected) in &TRIGGER_CASES {
        assert_eq!(madt::trigger_from_flags(flags), expected, "trigger_from_flags({flags:#06b})");
    }
}

/// `resolve_for_isa` must turn `ConformsToBus` into the ISA defaults
/// (edge-triggered, active-high -- ACPI 6.x 5.2.12.5) and leave every
/// other, already-concrete variant alone; for use by a future ISA-IRQ
/// consumer (the keyboard task) that needs a final polarity/trigger
/// decision before calling `arch::x86_64::ioapic::set_redirect`.
#[test_case]
fn madt_polarity_trigger_resolve_for_isa() {
    assert_eq!(madt::Polarity::ConformsToBus.resolve_for_isa(), madt::Polarity::ActiveHigh);
    assert_eq!(madt::Polarity::ActiveHigh.resolve_for_isa(), madt::Polarity::ActiveHigh);
    assert_eq!(madt::Polarity::ActiveLow.resolve_for_isa(), madt::Polarity::ActiveLow);

    assert_eq!(madt::Trigger::ConformsToBus.resolve_for_isa(), madt::Trigger::Edge);
    assert_eq!(madt::Trigger::Edge.resolve_for_isa(), madt::Trigger::Edge);
    assert_eq!(madt::Trigger::Level.resolve_for_isa(), madt::Trigger::Level);
}

/// This CPU's own local APIC ID (read straight from LAPIC hardware) must
/// appear among the MADT's own list of processor local APIC entries --
/// otherwise the MADT and the hardware it's supposed to describe
/// disagree about which CPU is even running this code.
#[test_case]
fn lapic_id_matches_a_madt_cpu() {
    let id = lapic::id();
    acpi::with_info(|info| {
        assert!(info.madt.cpus.iter().any(|c| c.apic_id == id), "LAPIC id {id} not found among MADT CPU entries");
    });
}
