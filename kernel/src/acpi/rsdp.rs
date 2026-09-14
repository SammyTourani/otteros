//! The Root System Description Pointer (ACPI 6.x spec 5.2.5): a small,
//! firmware-provided structure that starts the whole ACPI table chain.
//! Limine's `RsdpRequest` hands us its physical address directly (brief
//! M1-T5; DECISIONS.md D3); this module only validates and decodes it.

use super::checksum_is_zero;
use crate::mm::addr::PhysAddr;
use crate::mm::hhdm;

/// ACPI 1.0 RSDP length: signature(8) + checksum(1) + oem_id(6) +
/// revision(1) + rsdt_address(4).
const RSDP_V1_LEN: usize = 20;
/// ACPI 2.0+ RSDP length: the v1 fields above, plus length(4) +
/// xsdt_address(8) + extended_checksum(1) + reserved(3).
const RSDP_V2_LEN: usize = 36;

/// A validated, decoded RSDP.
pub struct Rsdp {
    pub revision: u8,
    pub rsdt_address: PhysAddr,
    /// Only `Some` for `revision >= 2` (ACPI 2.0+).
    pub xsdt_address: Option<PhysAddr>,
}

impl Rsdp {
    /// Validates and decodes the RSDP at physical address `phys`. `None`
    /// if the signature doesn't match, or either checksum (the mandatory
    /// 20-byte one, and -- for `revision >= 2` -- the extended 36-byte
    /// one) doesn't sum to zero.
    pub fn parse(phys: PhysAddr) -> Option<Self> {
        let virt = hhdm::phys_to_virt(phys).as_u64() as *const u8;
        // SAFETY: `phys` comes from Limine's own RSDP request, which
        // points at real, firmware-owned memory Limine's memory map
        // describes (typically `ACPI_RECLAIMABLE` or inside the EBDA/BIOS
        // read-only area, itself `RESERVED`) -- the kernel's own HHDM
        // (`mm::vmm::map_hhdm`) maps every memory-map-described region
        // regardless of type, plus every gap below 4 GiB no entry
        // mentions at all, so reading `RSDP_V2_LEN` bytes from `virt` is
        // always in bounds here regardless of whether this turns out to
        // be a v1 (20-byte) or v2 (36-byte) structure. Nothing else
        // writes to this memory (firmware tables, never touched again
        // after boot) or unmaps it, so the read is also data-race-free.
        let bytes = unsafe { core::slice::from_raw_parts(virt, RSDP_V2_LEN) };

        if &bytes[0..8] != b"RSD PTR " {
            return None;
        }
        if !checksum_is_zero(&bytes[0..RSDP_V1_LEN]) {
            return None;
        }

        let revision = bytes[15];
        let rsdt_address = u32::from_le_bytes(bytes[16..20].try_into().unwrap());

        let xsdt_address = if revision >= 2 {
            if !checksum_is_zero(&bytes[0..RSDP_V2_LEN]) {
                return None;
            }
            Some(PhysAddr::new(u64::from_le_bytes(bytes[24..32].try_into().unwrap())))
        } else {
            None
        };

        Some(Self { revision, rsdt_address: PhysAddr::new(u64::from(rsdt_address)), xsdt_address })
    }
}
