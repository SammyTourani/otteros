//! ACPI MCFG (Memory-Mapped Configuration) table parsing (brief M3-T1):
//! PCI configuration space (in QEMU: 0xb0000000) is accessed via Enhanced
//! Configuration Access Mechanism (ECAM), where bus:dev:func maps to an
//! offset inside a segment's ECAM base address.
//!
//! MCFG table layout (ACPI 6.x, 8.2):
//! - 36 byte header (standard SDT header)
//! - 8 byte reserved (all zero in practice)
//! - Variable number of 16-byte allocation structures, each:
//!   - 8 byte base address (physical, 64-bit)
//!   - 2 byte segment number
//!   - 1 byte bus start
//!   - 1 byte bus end
//!   - 4 byte reserved (all zero)

use super::sdt::SdtHeader;
use crate::mm::addr::PhysAddr;

#[derive(Clone, Debug)]
pub struct McfgAllocation {
    pub base_addr: PhysAddr,
    pub segment: u16,
    pub bus_start: u8,
    pub bus_end: u8,
}

pub struct Mcfg {
    pub allocations: alloc::vec::Vec<McfgAllocation>,
}

impl Mcfg {
    /// Parses an MCFG table. Returns None if the table is too short or
    /// malformed (not enough allocation structures).
    pub fn parse(header: &SdtHeader) -> Option<Self> {
        let body = header.body();

        // Skip 8-byte reserved field.
        if body.len() < 8 {
            return None;
        }
        let alloc_bytes = &body[8..];

        // Each allocation is 16 bytes.
        let mut allocations = alloc::vec::Vec::new();
        let (chunks, _remainder) = alloc_bytes.as_chunks::<16>();
        for chunk in chunks {
            let base_addr = PhysAddr::new(u64::from_le_bytes(chunk[0..8].try_into().unwrap()));
            let segment = u16::from_le_bytes(chunk[8..10].try_into().unwrap());
            let bus_start = chunk[10];
            let bus_end = chunk[11];
            // chunk[12..16] are reserved (not parsed).

            allocations.push(McfgAllocation { base_addr, segment, bus_start, bus_end });
        }

        Some(Mcfg { allocations })
    }
}
