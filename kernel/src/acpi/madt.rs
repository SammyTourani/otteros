//! The Multiple APIC Description Table ("APIC", ACPI 6.x spec 5.2.12):
//! the local APIC base address, one entry per CPU's local APIC, one per
//! I/O APIC, ISA IRQ -> GSI overrides, and LAPIC NMI wiring. Parsed from
//! the variable-length entry list that follows the MADT's own 8-byte
//! header (on top of the common 36-byte `SdtHeader`).

use alloc::vec::Vec;

use super::sdt::SdtHeader;
use crate::kprintln;
use crate::mm::addr::PhysAddr;

/// One CPU's Processor Local APIC entry (MADT type 0, ACPI 6.x table
/// 5-39).
#[derive(Clone, Copy, Debug)]
pub struct CpuEntry {
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    /// Flags bit 0: the CPU is usable. `false` almost always means a
    /// placeholder hot-pluggable slot that isn't actually present.
    pub enabled: bool,
}

/// One I/O APIC entry (MADT type 1, ACPI 6.x table 5-41).
#[derive(Clone, Copy, Debug)]
pub struct IoApicEntry {
    pub id: u8,
    pub address: PhysAddr,
    pub gsi_base: u32,
}

/// Polarity an interrupt line is wired for (MADT type 2's flags bits
/// 0-1). `ConformsToBus` means "whatever the bus default is" (active-low
/// for PCI, active-high for ISA); callers resolve it against the bus
/// themselves -- `resolve_for_isa` does exactly that for the one bus this
/// kernel currently cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Polarity {
    ConformsToBus,
    ActiveHigh,
    ActiveLow,
}

impl Polarity {
    /// Resolves `ConformsToBus` to the ISA bus default (active-high;
    /// ACPI 6.x 5.2.12.5, "Interrupt Source Override Structure": ISA
    /// interrupts are active-high by default). Any other variant passes
    /// through unchanged. For a future ISA-IRQ consumer (the keyboard
    /// task) that needs a concrete, final active-high/active-low decision
    /// -- never "whatever the bus says" -- before calling
    /// `arch::x86_64::ioapic::set_redirect`.
    pub fn resolve_for_isa(self) -> Self {
        match self {
            Polarity::ConformsToBus => Polarity::ActiveHigh,
            other => other,
        }
    }
}

/// Trigger mode an interrupt line is wired for (MADT type 2's flags bits
/// 2-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    ConformsToBus,
    Edge,
    Level,
}

impl Trigger {
    /// Resolves `ConformsToBus` to the ISA bus default (edge-triggered;
    /// ACPI 6.x 5.2.12.5). Any other variant passes through unchanged.
    /// See `Polarity::resolve_for_isa`'s docs.
    pub fn resolve_for_isa(self) -> Self {
        match self {
            Trigger::ConformsToBus => Trigger::Edge,
            other => other,
        }
    }
}

/// One ISA IRQ -> GSI remap (MADT type 2, ACPI 6.x table "Interrupt
/// Source Override Structure") -- e.g. QEMU/most PCs wire ISA IRQ 0 (the
/// PIT) to GSI 2, not GSI 0.
#[derive(Clone, Copy, Debug)]
pub struct IsaOverride {
    pub isa_irq: u8,
    pub gsi: u32,
    pub polarity: Polarity,
    pub trigger: Trigger,
}

/// The parsed MADT: everything `arch::x86_64::{lapic,ioapic,irq}` need to
/// program the machine's actual interrupt routing.
pub struct Madt {
    pub lapic_address: PhysAddr,
    pub cpus: Vec<CpuEntry>,
    pub ioapics: Vec<IoApicEntry>,
    pub overrides: Vec<IsaOverride>,
}

/// Decodes MPS INTI flags bits `1:0` (ACPI 6.x table 5-38): `00`
/// conforms to the bus's own default, `01` active-high, `10` reserved
/// (treated the same as "conforms", the conservative default), `11`
/// active-low. `pub` (not private) so `test_cases::acpi` -- a separate
/// crate -- can exercise all four encodings directly.
pub fn polarity_from_flags(flags: u16) -> Polarity {
    match flags & 0b11 {
        0b01 => Polarity::ActiveHigh,
        0b11 => Polarity::ActiveLow,
        _ => Polarity::ConformsToBus,
    }
}

/// Decodes MPS INTI flags bits `3:2`: `00` conforms to the bus, `01`
/// edge-triggered, `10` reserved (treated as "conforms"), `11`
/// level-triggered. `pub` for the same reason as `polarity_from_flags`.
pub fn trigger_from_flags(flags: u16) -> Trigger {
    match (flags >> 2) & 0b11 {
        0b01 => Trigger::Edge,
        0b11 => Trigger::Level,
        _ => Trigger::ConformsToBus,
    }
}

/// Byte length of the MADT's own fixed header (on top of the common
/// 36-byte `SdtHeader`): a 4-byte Local Interrupt Controller Address plus
/// a 4-byte Flags field, before the variable-length entry list starts.
const MIN_BODY_LEN: usize = 8;

/// Parses the MADT's own header fields plus every entry in its list.
/// `header` must be the `"APIC"`-signature table (checked by the caller,
/// `acpi::init`). An entry type this kernel doesn't otherwise care about
/// (or a future one it doesn't know about yet) is simply skipped, using
/// its own `length` byte -- never assumed to be any particular size.
///
/// `None` (logged, not fatal) if the table's body is shorter than the
/// fixed 8-byte header this must read before it can even look for
/// entries (kernel-review, M1-T5 fix #3) -- the caller (`acpi::init`)
/// treats that the same as "no MADT present" and halts, matching the
/// per-entry malformed-length handling the loop below already has.
pub fn parse(header: &SdtHeader) -> Option<Madt> {
    let body = header.body();
    if body.len() < MIN_BODY_LEN {
        kprintln!("[acpi] MADT table too short ({} bytes), skipped", body.len());
        return None;
    }
    let mut lapic_address = PhysAddr::new(u64::from(u32::from_le_bytes(body[0..4].try_into().unwrap())));
    // body[4..8] is the MADT's own flags (bit 0: dual-8259s present) --
    // not needed: `arch::x86_64::pic::remap_and_mask` always remaps and
    // masks the legacy PIC regardless of what this says.

    let mut cpus = Vec::new();
    let mut ioapics = Vec::new();
    let mut overrides = Vec::new();

    let mut offset = 8usize;
    while offset + 2 <= body.len() {
        let entry_type = body[offset];
        let entry_len = body[offset + 1] as usize;
        if entry_len < 2 || offset + entry_len > body.len() {
            break; // malformed trailing entry; stop rather than misread past the table.
        }
        let entry = &body[offset..offset + entry_len];

        match entry_type {
            0 if entry_len >= 8 => {
                let flags = u32::from_le_bytes(entry[4..8].try_into().unwrap());
                cpus.push(CpuEntry { acpi_processor_id: entry[2], apic_id: entry[3], enabled: flags & 1 != 0 });
            }
            1 if entry_len >= 12 => {
                ioapics.push(IoApicEntry {
                    id: entry[2],
                    address: PhysAddr::new(u64::from(u32::from_le_bytes(entry[4..8].try_into().unwrap()))),
                    gsi_base: u32::from_le_bytes(entry[8..12].try_into().unwrap()),
                });
            }
            2 if entry_len >= 10 => {
                let flags = u16::from_le_bytes(entry[8..10].try_into().unwrap());
                overrides.push(IsaOverride {
                    isa_irq: entry[3],
                    gsi: u32::from_le_bytes(entry[4..8].try_into().unwrap()),
                    polarity: polarity_from_flags(flags),
                    trigger: trigger_from_flags(flags),
                });
            }
            4 if entry_len >= 6 => {
                // Local APIC NMI: logged, not wired up -- there is no NMI
                // handler beyond the IST re-entry guard
                // (`arch::x86_64::trap`) until a later task needs one.
                kprintln!("[acpi] MADT: LAPIC NMI (processor {}, LINT{})", entry[2], entry[5]);
            }
            5 if entry_len >= 12 => {
                // Local APIC Address Override: a 64-bit LAPIC base,
                // superseding the 32-bit one this MADT's own header
                // reported.
                lapic_address = PhysAddr::new(u64::from_le_bytes(entry[4..12].try_into().unwrap()));
            }
            _ => {} // unknown/unused entry type; `entry_len` already let us skip it correctly.
        }

        offset += entry_len;
    }

    Some(Madt { lapic_address, cpus, ioapics, overrides })
}
