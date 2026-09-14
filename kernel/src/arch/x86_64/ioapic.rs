//! The I/O APIC (Intel 82093AA datasheet): MMIO through the HHDM, at
//! whatever base(s) the MADT reported. QEMU's `q35` machine (and every
//! real chipset this kernel expects to meet, D1) has exactly one, but
//! this handles more in case a later machine doesn't.

use alloc::vec::Vec;

use crate::acpi;
use crate::kprintln;
use crate::mm::hhdm;
use crate::sync::IrqMutex;

const REG_WINDOW_OFFSET: u64 = 0x10;
const IOAPICVER: u32 = 0x01;
/// Index of redirection-table entry 0's low dword; entry `n`'s low dword
/// is at `IOREDTBL_BASE + 2*n`, its high dword right after.
const IOREDTBL_BASE: u32 = 0x10;
/// Redirection-entry low-dword bit 16: masked.
const ENTRY_MASKED: u32 = 1 << 16;
/// No real I/O APIC (the original 82093AA or any of its successors this
/// kernel expects to meet, D1) has ever shipped with more than 24
/// redirection-table entries; a `max_redirection_entry` above this,
/// together with the all-ones `version` check below, is how this module
/// recognises "nothing answered at that MMIO address" rather than a real
/// register (kernel-review, M1-T5 fix #6).
const MAX_SANE_REDIRECTION_ENTRY: u8 = 24;
/// The version register reads back as all ones when nothing (a floating
/// bus, an address MADT reported but no device actually backs) answers
/// the read.
const BAD_VERSION_REGISTER: u32 = 0xFFFF_FFFF;

/// Redirection-entry polarity (low dword bit 13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Polarity {
    ActiveHigh,
    ActiveLow,
}

/// Redirection-entry trigger mode (low dword bit 15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Edge,
    Level,
}

struct IoApicRuntime {
    virt_base: u64,
    gsi_base: u32,
    max_redirection_entry: u8,
}

static IOAPICS: IrqMutex<Vec<IoApicRuntime>> = IrqMutex::new(Vec::new());

fn read(virt_base: u64, index: u32) -> u32 {
    let select = virt_base as *mut u32;
    let window = (virt_base + REG_WINDOW_OFFSET) as *mut u32;
    // SAFETY: `virt_base` is a live I/O APIC's HHDM-mapped MMIO base
    // (`init` only ever stores an address it just derived from a
    // MADT-reported physical one via `mm::hhdm::phys_to_virt`);
    // IOREGSEL/IOWIN is the documented two-register indirect access
    // protocol every I/O APIC implements, and `index` is always one of
    // this module's own small set of documented register indices.
    unsafe {
        core::ptr::write_volatile(select, index);
        core::ptr::read_volatile(window)
    }
}

fn write(virt_base: u64, index: u32, value: u32) {
    let select = virt_base as *mut u32;
    let window = (virt_base + REG_WINDOW_OFFSET) as *mut u32;
    // SAFETY: same reasoning as `read`, for a register/value this module
    // only ever writes in the documented format.
    unsafe {
        core::ptr::write_volatile(select, index);
        core::ptr::write_volatile(window, value);
    }
}

fn redirect_indices(n: u8) -> (u32, u32) {
    let low = IOREDTBL_BASE + 2 * u32::from(n);
    (low, low + 1)
}

fn write_entry_raw(virt_base: u64, n: u8, low: u32, high: u32) {
    let (low_idx, high_idx) = redirect_indices(n);
    // High dword first: if this entry was previously live (not the case
    // during `init`'s initial mask-everything pass, but true for any
    // later reprogramming through `set_redirect`), updating the
    // destination before the vector/mode/mask bits take effect avoids a
    // brief window where the old destination could receive an interrupt
    // already reprogrammed with the new vector.
    write(virt_base, high_idx, high);
    write(virt_base, low_idx, low);
}

/// Brings up every I/O APIC the MADT listed (brief M1-T5 step 6): reads
/// each one's version/max-redirection-entry count, masks every entry on
/// it (defence in depth -- real hardware/QEMU already reset unmasked
/// entries to masked, but nothing here should rely on that), and records
/// it for `set_redirect` to find later.
///
/// A controller whose version register reads back all-ones, or whose
/// reported `max_redirection_entry` exceeds `MAX_SANE_REDIRECTION_ENTRY`,
/// is treated as an init failure, not a panic (kernel-review, M1-T5 fix
/// #6): logged and left alone entirely (never written to with a
/// redirection-entry count this code doesn't trust, and never added to
/// `IOAPICS`, so `set_redirect` can never target it -- every GSI it would
/// have owned simply has no I/O APIC willing to route it, the same
/// "leave it masked" end state as if this controller had never been
/// brought up at all).
pub fn init() {
    let entries = acpi::with_info(|info| info.madt.ioapics.clone());
    let mut guard = IOAPICS.lock();
    for entry in entries {
        let virt_base = hhdm::phys_to_virt(entry.address).as_u64();
        let version = read(virt_base, IOAPICVER);
        let max_redirection_entry = (version >> 16) as u8;

        if version == BAD_VERSION_REGISTER || max_redirection_entry > MAX_SANE_REDIRECTION_ENTRY {
            kprintln!(
                "[ioapic] bad version register (id={} at 0x{:x}, version=0x{version:x}): leaving it masked/unused",
                entry.id,
                entry.address.as_u64()
            );
            continue;
        }

        for i in 0..=max_redirection_entry {
            write_entry_raw(virt_base, i, ENTRY_MASKED, 0);
        }

        kprintln!(
            "[ioapic] id={} at 0x{:x} gsi_base={} entries={}",
            entry.id,
            entry.address.as_u64(),
            entry.gsi_base,
            u32::from(max_redirection_entry) + 1
        );
        guard.push(IoApicRuntime { virt_base, gsi_base: entry.gsi_base, max_redirection_entry });
    }
}

/// Programs GSI `gsi`'s redirection entry: `vector` to deliver, the
/// destination `apic_id`, `polarity`/`trigger`, and whether it starts
/// masked. `arch::x86_64::irq::isa_to_gsi` is how a caller with an ISA
/// IRQ number (not a GSI) gets one of these in the first place.
///
/// # Panics
/// If no discovered I/O APIC's GSI range covers `gsi`.
pub fn set_redirect(gsi: u32, vector: u8, apic_id: u8, polarity: Polarity, trigger: Trigger, masked: bool) {
    let guard = IOAPICS.lock();
    let ioapic = guard
        .iter()
        .find(|a| gsi >= a.gsi_base && gsi <= a.gsi_base + u32::from(a.max_redirection_entry))
        .unwrap_or_else(|| panic!("ioapic::set_redirect: no I/O APIC owns GSI {gsi}"));
    let index = (gsi - ioapic.gsi_base) as u8;

    let mut low = u32::from(vector);
    if polarity == Polarity::ActiveLow {
        low |= 1 << 13;
    }
    if trigger == Trigger::Level {
        low |= 1 << 15;
    }
    if masked {
        low |= ENTRY_MASKED;
    }
    let high = u32::from(apic_id) << 24;

    write_entry_raw(ioapic.virt_base, index, low, high);
}

/// Test-only introspection: how many I/O APICs `init` discovered.
pub fn ioapic_count() -> usize {
    IOAPICS.lock().len()
}

/// Test-only introspection: the `index`-th discovered I/O APIC's maximum
/// redirection-table entry index (its entry count minus one).
///
/// # Panics
/// If `index >= ioapic_count()`.
pub fn max_redirection_entry(index: usize) -> u8 {
    IOAPICS.lock()[index].max_redirection_entry
}
