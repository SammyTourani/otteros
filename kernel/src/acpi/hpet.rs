//! The HPET table (IA-PC HPET spec 3.2.4, "HPET Description Table"):
//! parsed just far enough to find the HPET's own MMIO base address
//! (brief M1-T5 step 2). Actually programming the HPET is a later task --
//! DECISIONS.md D19 uses the LAPIC timer, calibrated against the legacy
//! PIT, as the scheduler tick and monotonic clock, not the HPET.

use super::sdt::SdtHeader;
use crate::kprintln;
use crate::mm::addr::PhysAddr;

/// Byte length of the fields this module reads: a 4-byte Event Timer
/// Block ID, then a 12-byte Generic Address Structure (ACPI 6.x table
/// 5-1: 1-byte address space ID, 1-byte register bit width, 1-byte
/// register bit offset, 1 reserved byte, then the 8-byte address itself)
/// -- so the address field sits at body offset `4 + 4 = 8`, ending at 16.
const MIN_BODY_LEN: usize = 16;

/// Parses the HPET table's base address. `None` (logged, not fatal --
/// matching the existing malformed-entry handling in `madt`) if the
/// table's body is shorter than the fixed fields this reads (kernel-
/// review, M1-T5 fix #3): a short/truncated `"HPET"` table is firmware
/// corruption, not something to read past the end of.
pub fn parse(header: &SdtHeader) -> Option<PhysAddr> {
    let body = header.body();
    if body.len() < MIN_BODY_LEN {
        kprintln!("[acpi] HPET table too short ({} bytes), skipped", body.len());
        return None;
    }
    let address = u64::from_le_bytes(body[8..16].try_into().unwrap());
    Some(PhysAddr::new(address))
}
