#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

mod crc;
mod gpt;
mod guid;
mod mbr;

pub use crc::crc32;
pub use guid::Guid;

use alloc::string::String;
use alloc::vec::Vec;

// ============= Public API =============

/// Error type for partition table operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartError {
    /// I/O error during read.
    Io,
    /// LBA 0 is not a valid MBR or GPT.
    NoTable,
    /// GPT is damaged and cannot be repaired.
    Corrupt,
    /// Sector size is not supported (must be 512, 1024, 2048, or 4096).
    Unsupported,
}

/// Trait for reading sectors from a block device.
pub trait SectorRead {
    /// Returns the sector size in bytes.
    fn sector_size(&self) -> usize;
    /// Returns the total number of sectors on the device.
    fn sector_count(&self) -> u64;
    /// Reads a sector at the given LBA into `buf`.
    /// `buf` must be exactly `sector_size()` bytes.
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ReadError>;
}

/// Opaque error type for read failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadError;

/// A partition table (either MBR or GPT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub scheme: Scheme,
    pub partitions: Vec<Partition>,
}

/// The partition scheme used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scheme {
    /// MBR with a disk signature.
    Mbr { disk_signature: u32 },
    /// GPT with a disk GUID. `used_backup` indicates if the backup header was used.
    Gpt { disk_guid: Guid, used_backup: bool },
}

/// A single partition entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub number: u32,
    pub first_lba: u64,
    pub sectors: u64,
    pub bootable: bool,
    pub name: String,
    pub kind: Kind,
}

/// Partition-specific information (MBR or GPT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Mbr { system_id: u8 },
    Gpt {
        type_guid: Guid,
        unique_guid: Guid,
        attributes: u64,
    },
}

/// Well-known GUIDs.
pub const EFI_SYSTEM: Guid = Guid([
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
]);

pub const BASIC_DATA: Guid = Guid([
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
]);

// ============= Table reading logic =============

/// Reads and parses a partition table from the given disk.
/// First checks for GPT (if any MBR entry has type 0xEE), otherwise reads as MBR.
pub fn read_table(disk: &mut dyn SectorRead) -> Result<Table, PartError> {
    let sector_size = disk.sector_size();

    // Validate sector size.
    if !is_power_of_two_in_range(sector_size, 512, 4096) {
        return Err(PartError::Unsupported);
    }

    // Read LBA 0.
    let mut buf = alloc::vec![0u8; sector_size];
    disk.read_sector(0, &mut buf).map_err(|_| PartError::Io)?;

    // Check if the disk ends in the MBR signature (0x55AA).
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

    // Check if this is a GPT disk (has an 0xEE entry in the MBR).
    let has_gpt_marker = (0..4).any(|i| buf[446 + i * 16 + 4] == 0xEE);

    if has_gpt_marker {
        // Read as GPT. Never fall back to MBR entries.
        let (disk_guid, used_backup, partitions) =
            gpt::read_gpt(disk)?;

        Ok(Table {
            scheme: Scheme::Gpt { disk_guid, used_backup },
            partitions,
        })
    } else {
        // Read as MBR.
        let (disk_signature, partitions) = mbr::read_mbr(disk)?;

        Ok(Table {
            scheme: Scheme::Mbr { disk_signature },
            partitions,
        })
    }
}

/// Checks if a value is a power of two between min and max (inclusive).
fn is_power_of_two_in_range(n: usize, min: usize, max: usize) -> bool {
    n >= min && n <= max && (n & (n - 1)) == 0
}
