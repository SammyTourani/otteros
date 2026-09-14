//! The FADT ("FACP", ACPI 6.x spec 5.2.9): parsed just far enough to
//! answer "does this machine's firmware say a legacy i8042 exists" (brief
//! M1-T6, `drivers::ps2`) via the IA-PC Boot Architecture Flags. Every
//! other FADT field (power management, ACPI enable/disable, ...) is a
//! later task's problem.

use super::sdt::SdtHeader;
use crate::kprintln;

/// Byte offset of the 2-byte IA-PC Boot Architecture Flags field within
/// the FADT's *body* (i.e. after the common 36-byte `SdtHeader`): ACPI
/// 6.x table 5-35 places it at byte 109 of the whole table, and
/// `SdtHeader::body()` already strips the header's own 36 bytes, so
/// `109 - 36 = 73` here.
const IAPC_BOOT_ARCH_OFFSET: usize = 73;
const IAPC_BOOT_ARCH_LEN: usize = 2;

/// IA-PC Boot Architecture Flags bit 1 (ACPI 6.x table 5-34): "8042" --
/// a legacy i8042 keyboard controller is present.
pub const IAPC_BOOT_ARCH_8042: u16 = 1 << 1;

/// The FADT's IA-PC Boot Architecture Flags, if this firmware's FADT
/// actually has them. This field was only added in the ACPI 2.0 FADT
/// (`header.revision() >= 2`); an ACPI 1.0 (revision 0/1) FADT's body
/// isn't even long enough to contain it. `None` here means exactly "this
/// field doesn't exist on this firmware" -- every caller
/// (`drivers::ps2::acpi_reports_i8042`) treats that the same as "assume a
/// legacy 8042 exists", matching every real ACPI 1.0-era machine, which
/// always has one.
///
/// Also `None` (logged, not fatal -- matching every other malformed-table
/// case in this module's siblings) if a revision-2-or-later FADT's body
/// is somehow shorter than this field's own offset: firmware corruption,
/// not something to read past the end of.
pub fn boot_arch_flags(header: &SdtHeader) -> Option<u16> {
    if header.revision() < 2 {
        return None;
    }
    let body = header.body();
    if body.len() < IAPC_BOOT_ARCH_OFFSET + IAPC_BOOT_ARCH_LEN {
        kprintln!("[acpi] FADT (revision {}) too short for IA-PC boot architecture flags ({} bytes)", header.revision(), body.len());
        return None;
    }
    Some(u16::from_le_bytes(body[IAPC_BOOT_ARCH_OFFSET..IAPC_BOOT_ARCH_OFFSET + 2].try_into().unwrap()))
}
