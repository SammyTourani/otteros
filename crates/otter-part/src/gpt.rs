//! GPT (GUID Partition Table) partition table parsing.

use crate::crc::crc32;
use crate::{Guid, Kind, PartError, Partition, SectorRead};
use alloc::string::String;
use alloc::vec::Vec;

const EFI_PART_SIGNATURE: &[u8] = b"EFI PART";
const MIN_HEADER_SIZE: usize = 92;
const MAX_ARRAY_SIZE: usize = 1024 * 1024; // 1 MiB

/// Header read from disk.
struct Header {
    first_usable: u64,
    last_usable: u64,
    disk_guid: Guid,
    array_lba: u64,
    num_entries: u32,
    entry_size: u32,
    array_crc: u32,
}

/// Reads and parses a GPT partition table.
/// Returns (disk_guid, used_backup, partitions) or an error.
pub fn read_gpt(disk: &mut dyn SectorRead) -> Result<(Guid, bool, Vec<Partition>), PartError> {
    let sector_size = disk.sector_size();
    let sector_count = disk.sector_count();

    if !is_power_of_two_in_range(sector_size, 512, 4096) {
        return Err(PartError::Unsupported);
    }

    // Try the primary header first.
    let primary = read_header(disk, 1, sector_size, sector_count)?;

    // Try to read the primary; if it succeeds, use it. Otherwise try the backup.
    if let Some(header) = primary {
        // Primary header is valid; try to read its array.
        if let Ok((disk_guid, partitions)) = read_array(disk, &header, sector_size) {
            return Ok((disk_guid, false, partitions));
        }
        // Primary array is corrupted; try the backup.
    }

    // Try the backup header.
    let last_lba = sector_count - 1;
    let backup = read_header(disk, last_lba, sector_size, sector_count)?;

    match backup {
        Some(header) => {
            let (disk_guid, partitions) = read_array(disk, &header, sector_size)
                .map_err(|_| PartError::Corrupt)?;
            Ok((disk_guid, true, partitions))
        }
        None => Err(PartError::Corrupt),
    }
}

/// Reads and parses the entry array for a given header.
fn read_array(
    disk: &mut dyn SectorRead,
    header: &Header,
    sector_size: usize,
) -> Result<(Guid, Vec<Partition>), PartError> {
    let entry_size = header.entry_size as usize;
    let num_entries = header.num_entries as usize;

    let array_bytes = entry_size.saturating_mul(num_entries);
    let array_sectors = array_bytes.div_ceil(sector_size);

    // Read the array into a buffer.
    let mut array_buf = alloc::vec![0u8; array_sectors * sector_size];
    for (i, sector_buf) in array_buf.chunks_mut(sector_size).enumerate() {
        disk.read_sector(header.array_lba + i as u64, sector_buf)
            .map_err(|_| PartError::Io)?;
    }

    // Verify the array CRC.
    if crc32(&array_buf[..array_bytes]) != header.array_crc {
        return Err(PartError::Corrupt);
    }

    // Parse partition entries.
    let mut partitions = Vec::new();
    for (idx, entry_bytes) in array_buf[..array_bytes].chunks(entry_size).enumerate() {
        let entry = parse_entry(entry_bytes, idx as u32, header.first_usable, header.last_usable)?;
        if let Some(p) = entry {
            partitions.push(p);
        }
    }

    Ok((header.disk_guid, partitions))
}

/// Tries to read and validate a GPT header at the given LBA.
fn read_header(
    disk: &mut dyn SectorRead,
    lba: u64,
    sector_size: usize,
    sector_count: u64,
) -> Result<Option<Header>, PartError> {
    let mut buf = alloc::vec![0u8; sector_size];
    disk.read_sector(lba, &mut buf).map_err(|_| PartError::Io)?;

    // Check signature.
    if &buf[0..8] != EFI_PART_SIGNATURE {
        return Ok(None);
    }

    // Read header size (bytes 12-15).
    let header_size = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]) as usize;

    // Validate header size.
    if header_size < MIN_HEADER_SIZE || header_size > sector_size {
        return Ok(None);
    }

    // Verify header CRC (computed over header_size bytes, with CRC field zeroed).
    let crc_stored = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
    let mut crc_buf = buf[..header_size].to_vec();
    crc_buf[16..20].fill(0);
    let crc_computed = crc32(&crc_buf);
    if crc_computed != crc_stored {
        return Ok(None);
    }

    // Read MyLBA (bytes 24-31) and verify it matches the LBA we read from.
    let my_lba = u64::from_le_bytes([
        buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31],
    ]);
    if my_lba != lba {
        return Ok(None);
    }

    // Read fields from header.
    let _alt_lba = u64::from_le_bytes([
        buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39],
    ]);
    let first_usable = u64::from_le_bytes([
        buf[40], buf[41], buf[42], buf[43], buf[44], buf[45], buf[46], buf[47],
    ]);
    let last_usable = u64::from_le_bytes([
        buf[48], buf[49], buf[50], buf[51], buf[52], buf[53], buf[54], buf[55],
    ]);

    // Validate usable range.
    if first_usable > last_usable || last_usable >= sector_count {
        return Ok(None);
    }

    // Read disk GUID (bytes 56-71).
    let mut disk_guid_bytes = [0u8; 16];
    disk_guid_bytes.copy_from_slice(&buf[56..72]);
    let disk_guid = Guid(disk_guid_bytes);

    // Read array info.
    let array_lba = u64::from_le_bytes([
        buf[72], buf[73], buf[74], buf[75], buf[76], buf[77], buf[78], buf[79],
    ]);
    let num_entries = u32::from_le_bytes([buf[80], buf[81], buf[82], buf[83]]);
    let entry_size = u32::from_le_bytes([buf[84], buf[85], buf[86], buf[87]]);
    let array_crc = u32::from_le_bytes([buf[88], buf[89], buf[90], buf[91]]);

    // Validate entry size: must be power of two and >= 128.
    if !is_power_of_two_in_range(entry_size as usize, 128, 0x80000000) {
        return Ok(None);
    }

    // Validate array size (at most 1 MiB).
    let array_bytes = entry_size as usize * num_entries as usize;
    if array_bytes > MAX_ARRAY_SIZE {
        return Ok(None);
    }

    // Validate array location: must be entirely outside usable range and inside the disk.
    let array_sectors = array_bytes.div_ceil(sector_size);
    let array_end = array_lba.saturating_add(array_sectors as u64);

    if array_lba < first_usable {
        // Array is before usable range; it must end before first_usable.
        if array_end > first_usable {
            return Ok(None);
        }
    } else if array_lba <= last_usable {
        // Array starts inside usable range: invalid.
        return Ok(None);
    } else {
        // Array is after usable range; it must fit inside the disk.
        if array_end > sector_count {
            return Ok(None);
        }
    }

    Ok(Some(Header {
        first_usable,
        last_usable,
        disk_guid,
        array_lba,
        num_entries,
        entry_size,
        array_crc,
    }))
}

/// Parses a single GPT entry.
fn parse_entry(
    bytes: &[u8],
    idx: u32,
    first_usable: u64,
    last_usable: u64,
) -> Result<Option<Partition>, PartError> {
    // Read type GUID (first 16 bytes).
    let mut type_guid_bytes = [0u8; 16];
    type_guid_bytes.copy_from_slice(&bytes[0..16]);
    let type_guid = Guid(type_guid_bytes);

    // Skip empty entries (zero type GUID).
    if type_guid == Guid([0; 16]) {
        return Ok(None);
    }

    // Read unique GUID.
    let mut unique_guid_bytes = [0u8; 16];
    unique_guid_bytes.copy_from_slice(&bytes[16..32]);
    let unique_guid = Guid(unique_guid_bytes);

    // Read first and last LBA.
    let first_lba = u64::from_le_bytes([
        bytes[32], bytes[33], bytes[34], bytes[35], bytes[36], bytes[37], bytes[38], bytes[39],
    ]);
    let last_lba = u64::from_le_bytes([
        bytes[40], bytes[41], bytes[42], bytes[43], bytes[44], bytes[45], bytes[46], bytes[47],
    ]);

    // Read attributes.
    let attributes = u64::from_le_bytes([
        bytes[48], bytes[49], bytes[50], bytes[51], bytes[52], bytes[53], bytes[54], bytes[55],
    ]);

    // Skip partitions outside the usable range or with invalid bounds.
    if first_lba < first_usable || last_lba > last_usable || first_lba > last_lba {
        return Ok(None);
    }

    // Read and decode name (UTF-16LE, up to 36 code units, at bytes 56..128).
    let name_bytes = &bytes[56..128];
    let name = decode_utf16_name(name_bytes);

    // Bootable is attribute bit 2.
    let bootable = (attributes & 0x0000_0000_0000_0004) != 0;

    Ok(Some(Partition {
        number: idx + 1,
        first_lba,
        sectors: last_lba - first_lba + 1,
        bootable,
        name,
        kind: Kind::Gpt {
            type_guid,
            unique_guid,
            attributes,
        },
    }))
}

/// Decodes UTF-16LE name from 72 bytes, stopping at the first NUL.
fn decode_utf16_name(bytes: &[u8]) -> String {
    let mut code_units = Vec::new();
    for pair in bytes.chunks(2) {
        if pair.len() == 2 {
            let cu = u16::from_le_bytes([pair[0], pair[1]]);
            if cu == 0 {
                break;
            }
            code_units.push(cu);
        }
    }

    // Decode UTF-16 with surrogate pair handling.
    let mut result = String::new();
    let mut i = 0;
    while i < code_units.len() {
        let cu = code_units[i];
        if (cu & 0xFC00) == 0xD800 {
            // High surrogate: try to pair with a low surrogate.
            if i + 1 < code_units.len() {
                let low = code_units[i + 1];
                if (low & 0xFC00) == 0xDC00 {
                    // Valid surrogate pair.
                    let high = cu as u32;
                    let low = low as u32;
                    let codepoint =
                        0x10000 + (((high & 0x03FF) << 10) | (low & 0x03FF));
                    if let Some(c) = char::from_u32(codepoint) {
                        result.push(c);
                    } else {
                        result.push('\u{FFFD}');
                    }
                    i += 2;
                    continue;
                }
            }
            // Unpaired high surrogate.
            result.push('\u{FFFD}');
            i += 1;
        } else if (cu & 0xFC00) == 0xDC00 {
            // Unpaired low surrogate.
            result.push('\u{FFFD}');
            i += 1;
        } else if cu == 0xFFFE || cu == 0xFFFF {
            // Invalid code unit.
            result.push('\u{FFFD}');
            i += 1;
        } else if let Some(c) = char::from_u32(cu as u32) {
            result.push(c);
            i += 1;
        } else {
            result.push('\u{FFFD}');
            i += 1;
        }
    }

    result
}

/// Checks if a value is a power of two and within a given range.
fn is_power_of_two_in_range(n: usize, min: usize, max: usize) -> bool {
    n >= min && n <= max && (n & (n - 1)) == 0
}
