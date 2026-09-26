//! MBR (Master Boot Record) partition table parsing.

use crate::{Kind, PartError, Partition, SectorRead};
use alloc::string::String;
use alloc::vec::Vec;

/// Reads and parses an MBR partition table from LBA 0.
/// Returns (disk_signature, partitions) or an error.
pub fn read_mbr(disk: &mut dyn SectorRead) -> Result<(u32, Vec<Partition>), PartError> {
    let sector_size = disk.sector_size();
    let sector_count = disk.sector_count();

    if !is_power_of_two_in_range(sector_size, 512, 4096) {
        return Err(PartError::Unsupported);
    }

    let mut buf = alloc::vec![0u8; sector_size];
    disk.read_sector(0, &mut buf).map_err(|_| PartError::Io)?;

    // Check for MBR signature (0x55AA at bytes 510-511).
    if buf[510] != 0x55 || buf[511] != 0xAA {
        return Err(PartError::NoTable);
    }

    // Check boot indicators: all must be 0x00 or 0x80.
    for i in 0..4 {
        let boot = buf[446 + i * 16];
        if boot != 0x00 && boot != 0x80 {
            return Err(PartError::NoTable);
        }
    }

    // Read disk signature from bytes 440-443.
    let disk_signature = u32::from_le_bytes([buf[440], buf[441], buf[442], buf[443]]);

    // Parse partition entries.
    let mut partitions = Vec::new();
    for slot in 0..4 {
        let base = 446 + slot * 16;
        let boot = buf[base];
        let start = u32::from_le_bytes([buf[base + 8], buf[base + 9], buf[base + 10], buf[base + 11]]) as u64;
        let size = u32::from_le_bytes([buf[base + 12], buf[base + 13], buf[base + 14], buf[base + 15]]) as u64;
        let system_id = buf[base + 4];

        // Skip empty entries.
        if system_id == 0 || start == 0 || size == 0 {
            continue;
        }

        // Skip partitions that run past the end of the disk.
        let end = start.saturating_add(size);
        if end > sector_count {
            continue;
        }

        partitions.push(Partition {
            number: (slot + 1) as u32,
            first_lba: start,
            sectors: size,
            bootable: boot == 0x80,
            name: String::new(),
            kind: Kind::Mbr { system_id },
        });
    }

    Ok((disk_signature, partitions))
}

/// Checks if a value is a power of two between min and max (inclusive).
fn is_power_of_two_in_range(n: usize, min: usize, max: usize) -> bool {
    n >= min && n <= max && (n & (n - 1)) == 0
}
