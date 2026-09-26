//! Acceptance oracle for brief M10-T0c (otter-part: MBR and GPT partition tables), written by the
//! orchestrator. The crate must pass this file unchanged. GPT disks are built here by an
//! independent writer (UEFI 2.10 section 5.3) with a bitwise CRC-32; the hybrid MBR is the real one
//! xorriso and `limine bios-install` wrote into build/otteros.iso.
//!
//! API this file relies on:
//!   struct ReadError; trait SectorRead { fn sector_size(&self) -> usize; fn sector_count(&self) -> u64;
//!     fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ReadError>; }
//!   crc32(&[u8]) -> u32 (IEEE 802.3, as GPT uses it)
//!   Guid(pub [u8; 16]) in on-disk byte order (Debug, Clone, Copy, PartialEq, Eq), Guid::parse(&str)
//!     -> Option<Guid>, Display as the canonical upper-case text; consts EFI_SYSTEM, BASIC_DATA
//!   read_table(&mut dyn SectorRead) -> Result<Table, PartError>
//!   Table { scheme: Scheme, partitions: Vec<Partition> }
//!   Scheme::{Mbr { disk_signature: u32 }, Gpt { disk_guid: Guid, used_backup: bool }}
//!   Partition { number: u32, first_lba: u64, sectors: u64, bootable: bool, name: String, kind: Kind }
//!   Kind::{Mbr { system_id: u8 }, Gpt { type_guid: Guid, unique_guid: Guid, attributes: u64 }}
//!   PartError::{Io, NoTable, Corrupt, Unsupported}   (Table, Scheme, Partition, Kind, PartError:
//!     Debug, Clone, PartialEq, Eq)
//!
//! Semantics:
//! - Sector sizes other than a power of two in 512..=4096 are Unsupported. Any read error is Io.
//! - LBA 0 is an MBR when it ends in 55 AA and every entry's boot indicator is 0x00 or 0x80 (a FAT
//!   boot sector of a partitionless stick fails this); otherwise NoTable.
//! - When any MBR entry has type 0xEE the disk is GPT: the primary header (LBA 1) is used if valid,
//!   else the backup header at the last LBA; if neither is valid the result is Corrupt (never the
//!   protective MBR's entries). Without an 0xEE entry the MBR's entries are the table, even if a
//!   stale GPT is present.
//! - A GPT header is valid when: signature "EFI PART"; 92 <= header size <= sector size; its CRC-32
//!   (over header-size bytes with the CRC field zeroed) matches; MyLBA is the LBA it was read from;
//!   first usable <= last usable < sector count; entry size is a power of two >= 128; the entry
//!   array is at most 1 MiB (checked before reading it), lies inside the disk and entirely outside
//!   the usable range, and its CRC-32 matches. The backup header is always read from the last LBA.
//! - GPT partitions: entries with a zero type GUID are empty; entries whose first LBA is below the
//!   first usable LBA, whose last LBA is above the last usable LBA, or whose first LBA exceeds the
//!   last are skipped; `number` is the entry index + 1; `sectors` = last - first + 1; `bootable` is
//!   attribute bit 2 (legacy BIOS bootable); `name` is the UTF-16LE name up to the first NUL (at
//!   most 36 code units, unpaired surrogates as U+FFFD).
//! - MBR partitions: entries of type 0, with a start of 0 or a size of 0 are skipped; `number` is the
//!   slot + 1; `bootable` is the 0x80 indicator; `name` is empty; entries that do not fit inside the
//!   disk are skipped. Extended partitions are reported as they are, not walked. `disk_signature` is
//!   the u32 at byte 440.

use otter_part::{crc32, read_table, Guid, Kind, PartError, Partition, ReadError, Scheme, SectorRead, BASIC_DATA, EFI_SYSTEM};

// ---------------------------------------------------------------------------------------------
// An in-memory disk and an independent GPT writer.
// ---------------------------------------------------------------------------------------------

struct Disk {
    sector: usize,
    data: Vec<u8>,
    fail: Option<u64>,
    reads: usize,
}

impl SectorRead for Disk {
    fn sector_size(&self) -> usize {
        self.sector
    }
    fn sector_count(&self) -> u64 {
        (self.data.len() / self.sector) as u64
    }
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ReadError> {
        self.reads += 1;
        let off = (lba as usize).checked_mul(self.sector).ok_or(ReadError)?;
        if Some(lba) == self.fail || buf.len() != self.sector || off + self.sector > self.data.len() {
            return Err(ReadError);
        }
        buf.copy_from_slice(&self.data[off..off + self.sector]);
        Ok(())
    }
}

fn crc32_ref(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn guid_bytes(text: &str) -> [u8; 16] {
    let g: Vec<&str> = text.split('-').collect();
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&u32::from_str_radix(g[0], 16).unwrap().to_le_bytes());
    out[4..6].copy_from_slice(&u16::from_str_radix(g[1], 16).unwrap().to_le_bytes());
    out[6..8].copy_from_slice(&u16::from_str_radix(g[2], 16).unwrap().to_le_bytes());
    out[8..10].copy_from_slice(&u16::from_str_radix(g[3], 16).unwrap().to_be_bytes());
    out[10..16].copy_from_slice(&u64::from_str_radix(g[4], 16).unwrap().to_be_bytes()[2..]);
    out
}

const ESP: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
const DATA: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";
const DISK_GUID: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";
const U1: &str = "11111111-2222-3333-4444-555555555555";
const U2: &str = "A0A1A2A3-B0B1-C0C1-D0D1-E0E1E2E3E4E5";

struct Part {
    slot: usize,
    type_guid: &'static str,
    unique: &'static str,
    first: u64,
    last: u64,
    attrs: u64,
    name: &'static str,
}

fn usable(sector: usize, sectors: u64) -> (u64, u64) {
    let array_sectors = (128 * 128usize).div_ceil(sector) as u64;
    (2 + array_sectors, sectors - 2 - array_sectors)
}

fn put(disk: &mut Disk, lba: u64, bytes: &[u8]) {
    let o = lba as usize * disk.sector;
    disk.data[o..o + bytes.len()].copy_from_slice(bytes);
}

fn protective_mbr(disk: &mut Disk) {
    let sectors = disk.sector_count();
    let e = 446;
    disk.data[e..e + 16].copy_from_slice(&[0; 16]);
    disk.data[e + 1..e + 4].copy_from_slice(&[0x00, 0x02, 0x00]);
    disk.data[e + 4] = 0xEE;
    disk.data[e + 5..e + 8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    disk.data[e + 8..e + 12].copy_from_slice(&1u32.to_le_bytes());
    disk.data[e + 12..e + 16].copy_from_slice(&((sectors - 1).min(0xFFFF_FFFF) as u32).to_le_bytes());
    disk.data[510] = 0x55;
    disk.data[511] = 0xAA;
}

fn gpt_disk(sector: usize, sectors: u64, parts: &[Part]) -> Disk {
    let mut disk = Disk { sector, data: vec![0u8; sector * sectors as usize], fail: None, reads: 0 };
    protective_mbr(&mut disk);
    let mut array = vec![0u8; 128 * 128];
    for p in parts {
        let o = p.slot * 128;
        array[o..o + 16].copy_from_slice(&guid_bytes(p.type_guid));
        array[o + 16..o + 32].copy_from_slice(&guid_bytes(p.unique));
        array[o + 32..o + 40].copy_from_slice(&p.first.to_le_bytes());
        array[o + 40..o + 48].copy_from_slice(&p.last.to_le_bytes());
        array[o + 48..o + 56].copy_from_slice(&p.attrs.to_le_bytes());
        for (k, u) in p.name.encode_utf16().enumerate() {
            array[o + 56 + 2 * k..o + 58 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
    }
    let array_sectors = (128 * 128usize).div_ceil(sector) as u64;
    let (first_usable, last_usable) = usable(sector, sectors);
    let last = sectors - 1;
    let backup_array = last - array_sectors;
    let array_crc = crc32_ref(&array);
    let header = |my: u64, alt: u64, entries_lba: u64| -> Vec<u8> {
        let mut h = vec![0u8; 92];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&my.to_le_bytes());
        h[32..40].copy_from_slice(&alt.to_le_bytes());
        h[40..48].copy_from_slice(&first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[56..72].copy_from_slice(&guid_bytes(DISK_GUID));
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&array_crc.to_le_bytes());
        let crc = crc32_ref(&h);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    };
    let primary = header(1, last, 2);
    let backup = header(last, 1, backup_array);
    put(&mut disk, 1, &primary);
    put(&mut disk, 2, &array);
    put(&mut disk, backup_array, &array);
    put(&mut disk, last, &backup);
    disk
}

/// Rewrites a u32/u64 field of the header at `lba` and recomputes its CRC, so only the semantic
/// check under test can fail.
fn patch_header(disk: &mut Disk, lba: u64, offset: usize, value: &[u8]) {
    let o = lba as usize * disk.sector;
    disk.data[o + offset..o + offset + value.len()].copy_from_slice(value);
    let size = u32::from_le_bytes(disk.data[o + 12..o + 16].try_into().unwrap()) as usize;
    let size = size.clamp(92, disk.sector);
    disk.data[o + 16..o + 20].copy_from_slice(&[0; 4]);
    let crc = crc32_ref(&disk.data[o..o + size]);
    disk.data[o + 16..o + 20].copy_from_slice(&crc.to_le_bytes());
}

fn standard_parts(sector: usize, sectors: u64) -> Vec<Part> {
    let (first_usable, last_usable) = usable(sector, sectors);
    let split = first_usable + (last_usable - first_usable) / 4;
    vec![
        Part { slot: 0, type_guid: ESP, unique: U1, first: first_usable, last: split - 1, attrs: 0, name: "EFI system" },
        Part { slot: 1, type_guid: DATA, unique: U2, first: split, last: last_usable, attrs: 4, name: "OTTERDATA 🦦" },
    ]
}

fn expected(sector: usize, sectors: u64) -> Vec<Partition> {
    standard_parts(sector, sectors)
        .iter()
        .map(|p| Partition {
            number: p.slot as u32 + 1,
            first_lba: p.first,
            sectors: p.last - p.first + 1,
            bootable: p.attrs & 4 != 0,
            name: p.name.into(),
            kind: Kind::Gpt { type_guid: Guid(guid_bytes(p.type_guid)), unique_guid: Guid(guid_bytes(p.unique)), attributes: p.attrs },
        })
        .collect()
}

fn gpt(disk_used_backup: bool) -> Scheme {
    Scheme::Gpt { disk_guid: Guid(guid_bytes(DISK_GUID)), used_backup: disk_used_backup }
}

// ---------------------------------------------------------------------------------------------

#[test]
fn crc32_is_the_ieee_polynomial() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926, "the standard check value");
    assert_eq!(crc32(b""), 0);
    let data: Vec<u8> = (0..1_000_003u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    assert_eq!(crc32(&data), crc32_ref(&data));
}

#[test]
fn guids() {
    assert_eq!(EFI_SYSTEM.0, [0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B]);
    assert_eq!(Guid::parse(ESP), Some(EFI_SYSTEM));
    assert_eq!(Guid::parse(&DATA.to_lowercase()), Some(BASIC_DATA), "hex digits in either case");
    assert_eq!(BASIC_DATA.to_string(), DATA);
    assert_eq!(Guid(guid_bytes(U2)).to_string(), U2);
    for bad in ["", "C12A7328F81F11D2BA4B00A0C93EC93B", "C12A7328-F81F-11D2-BA4B-00A0C93EC93", "C12A7328-F81F-11D2-BA4B-00A0C93EC93BB", "G12A7328-F81F-11D2-BA4B-00A0C93EC93B", "C12A7328-F81F11D2-BA4B-00A0C93EC93B-"] {
        assert_eq!(Guid::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn gpt_on_512_byte_sectors() {
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    let t = read_table(&mut disk).unwrap();
    assert_eq!(t.scheme, gpt(false));
    assert_eq!(t.partitions, expected(512, 16_384));
    assert_eq!(t.partitions[1].name, "OTTERDATA 🦦", "names decode surrogate pairs");
    assert!(t.partitions[1].bootable && !t.partitions[0].bootable);
}

#[test]
fn gpt_on_4096_and_2048_byte_sectors() {
    for (sector, sectors) in [(4096usize, 2048u64), (2048, 4096)] {
        let mut disk = gpt_disk(sector, sectors, &standard_parts(sector, sectors));
        let t = read_table(&mut disk).unwrap();
        assert_eq!(t.scheme, gpt(false), "{sector}-byte sectors");
        assert_eq!(t.partitions, expected(sector, sectors), "{sector}-byte sectors");
    }
}

#[test]
fn sparse_slots_keep_their_numbers() {
    let (first_usable, last_usable) = usable(512, 8192);
    let parts = [
        Part { slot: 3, type_guid: DATA, unique: U1, first: first_usable, last: first_usable + 99, attrs: 0, name: "four" },
        Part { slot: 127, type_guid: ESP, unique: U2, first: first_usable + 100, last: last_usable, attrs: 0, name: "last" },
    ];
    let mut disk = gpt_disk(512, 8192, &parts);
    let t = read_table(&mut disk).unwrap();
    let numbers: Vec<u32> = t.partitions.iter().map(|p| p.number).collect();
    assert_eq!(numbers, [4, 128]);
}

#[test]
fn backup_is_used_when_the_primary_is_damaged() {
    // A flipped byte in the primary header (CRC mismatch).
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    disk.data[512 + 60] ^= 0x01;
    let t = read_table(&mut disk).unwrap();
    assert_eq!((t.scheme, t.partitions), (gpt(true), expected(512, 16_384)));

    // A flipped byte in the primary entry array (array CRC mismatch).
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    disk.data[2 * 512 + 128 + 60] ^= 0x20;
    let t = read_table(&mut disk).unwrap();
    assert_eq!((t.scheme, t.partitions), (gpt(true), expected(512, 16_384)), "names come from the backup array");

    // An unreadable-looking but CRC-correct primary: each semantic check on its own.
    let cases: [(usize, Vec<u8>, &str); 8] = [
        (24, 5u64.to_le_bytes().to_vec(), "MyLBA is not 1"),
        (12, 91u32.to_le_bytes().to_vec(), "header smaller than 92 bytes"),
        (12, 600u32.to_le_bytes().to_vec(), "header larger than the sector"),
        (84, 100u32.to_le_bytes().to_vec(), "entry size not a power of two"),
        (84, 64u32.to_le_bytes().to_vec(), "entry size below 128"),
        (48, 16_384u64.to_le_bytes().to_vec(), "last usable LBA beyond the disk"),
        (72, 100u64.to_le_bytes().to_vec(), "entry array inside the usable range"),
        (72, 16_380u64.to_le_bytes().to_vec(), "entry array past the end of the disk"),
    ];
    for (offset, value, what) in cases {
        let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
        patch_header(&mut disk, 1, offset, &value);
        let t = read_table(&mut disk).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert_eq!(t.scheme, gpt(true), "{what}");
        assert_eq!(t.partitions, expected(512, 16_384), "{what}");
    }

    // A huge entry array is refused before it is read.
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    patch_header(&mut disk, 1, 80, &100_000u32.to_le_bytes());
    let t = read_table(&mut disk).unwrap();
    assert_eq!(t.scheme, gpt(true));
    assert!(disk.reads < 1000, "the 12.8 MB primary array must not be read ({} reads)", disk.reads);
}

#[test]
fn both_copies_damaged_is_corrupt() {
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    disk.data[512 + 60] ^= 0x01;
    let last = 16_383 * 512;
    disk.data[last + 60] ^= 0x01;
    assert_eq!(read_table(&mut disk), Err(PartError::Corrupt));

    // A damaged primary and a backup whose MyLBA is wrong.
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    disk.data[512 + 60] ^= 0x01;
    patch_header(&mut disk, 16_383, 24, &16_382u64.to_le_bytes());
    assert_eq!(read_table(&mut disk), Err(PartError::Corrupt));

    // A protective MBR with no GPT at all.
    let mut disk = Disk { sector: 512, data: vec![0; 512 * 4096], fail: None, reads: 0 };
    protective_mbr(&mut disk);
    assert_eq!(read_table(&mut disk), Err(PartError::Corrupt), "never fall back to the 0xEE entry");
}

#[test]
fn entries_outside_the_usable_range_are_skipped() {
    let (first_usable, last_usable) = usable(512, 8192);
    let parts = [
        Part { slot: 0, type_guid: DATA, unique: U1, first: first_usable - 1, last: first_usable + 10, attrs: 0, name: "too early" },
        Part { slot: 1, type_guid: DATA, unique: U1, first: first_usable + 20, last: first_usable + 10, attrs: 0, name: "backwards" },
        Part { slot: 2, type_guid: DATA, unique: U1, first: last_usable - 10, last: last_usable + 1, attrs: 0, name: "too late" },
        Part { slot: 3, type_guid: DATA, unique: U2, first: first_usable, last: last_usable, attrs: 0, name: "fine" },
    ];
    let mut disk = gpt_disk(512, 8192, &parts);
    let t = read_table(&mut disk).unwrap();
    let names: Vec<(u32, &str)> = t.partitions.iter().map(|p| (p.number, p.name.as_str())).collect();
    assert_eq!(names, [(4, "fine")]);
}

/// Bytes 440..512 of the MBR xorriso + `limine bios-install` wrote into build/otteros.iso.
const ISO_MBR_TAIL: [u8; 72] = [
    0x54, 0x9F, 0x88, 0x3E, 0x00, 0x00, 0x80, 0x01, 0x02, 0x00, 0x07, 0x02, 0x0E, 0x00, 0x40, 0x00, 0x00, 0x00,
    0x4C, 0x00, 0x00, 0x00, 0x00, 0x02, 0x0F, 0x00, 0xEF, 0x5D, 0x29, 0x00, 0x8C, 0x00, 0x00, 0x00, 0x80, 0x16,
    0x00, 0x00, 0x00, 0x5D, 0x2A, 0x00, 0x07, 0xAA, 0x39, 0x01, 0x0C, 0x17, 0x00, 0x00, 0xC4, 0x51, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x55, 0xAA,
];

fn mbr_part(number: u32, first: u64, sectors: u64, system_id: u8, bootable: bool) -> Partition {
    Partition { number, first_lba: first, sectors, bootable, name: String::new(), kind: Kind::Mbr { system_id } }
}

#[test]
fn the_isos_hybrid_mbr() {
    let mut disk = Disk { sector: 512, data: vec![0; 512 * 26_880], fail: None, reads: 0 };
    disk.data[..440].fill(0xFA); // boot code
    disk.data[440..512].copy_from_slice(&ISO_MBR_TAIL);
    let t = read_table(&mut disk).unwrap();
    assert_eq!(t.scheme, Scheme::Mbr { disk_signature: 0x3E88_9F54 });
    assert_eq!(t.partitions, [mbr_part(1, 64, 76, 0x07, true), mbr_part(2, 140, 5760, 0xEF, false), mbr_part(3, 5900, 20_932, 0x07, false)]);
}

#[test]
fn which_table_wins() {
    // A legacy MBR (no 0xEE) over a stale GPT: the MBR is the table.
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    disk.data[446..462].copy_from_slice(&[0x80, 0, 0, 0, 0x0C, 0, 0, 0, 0x00, 0x08, 0, 0, 0x00, 0x38, 0, 0]);
    let t = read_table(&mut disk).unwrap();
    assert_eq!(t.scheme, Scheme::Mbr { disk_signature: 0 });
    assert_eq!(t.partitions, [mbr_part(1, 2048, 14_336, 0x0C, true)]);

    // A hybrid MBR (an 0xEE entry next to a FAT32 one) over a valid GPT: the GPT is the table.
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    let fat = [0x00, 0, 0, 0, 0x0C, 0, 0, 0, 0x00, 0x08, 0, 0, 0x00, 0x38, 0, 0];
    disk.data[462..478].copy_from_slice(&fat);
    let t = read_table(&mut disk).unwrap();
    assert_eq!((t.scheme, t.partitions), (gpt(false), expected(512, 16_384)));
}

#[test]
fn not_a_partition_table() {
    let mut zeros = Disk { sector: 512, data: vec![0; 512 * 64], fail: None, reads: 0 };
    assert_eq!(read_table(&mut zeros), Err(PartError::NoTable));

    // A FAT32 boot sector of a partitionless stick also ends in 55 AA; its boot code sits where
    // the partition entries would be.
    let mut superfloppy = Disk { sector: 512, data: vec![0; 512 * 64], fail: None, reads: 0 };
    superfloppy.data[..11].copy_from_slice(&[0xEB, 0x58, 0x90, b'M', b'S', b'D', b'O', b'S', b'5', b'.', b'0']);
    superfloppy.data[446..510].fill(0x33);
    superfloppy.data[510] = 0x55;
    superfloppy.data[511] = 0xAA;
    assert_eq!(read_table(&mut superfloppy), Err(PartError::NoTable));

    // A valid but empty MBR, and entries that are present but empty.
    let mut empty = Disk { sector: 512, data: vec![0; 512 * 64], fail: None, reads: 0 };
    empty.data[510] = 0x55;
    empty.data[511] = 0xAA;
    empty.data[446..462].copy_from_slice(&[0x00, 0, 0, 0, 0x0C, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0]); // start 0
    empty.data[462..478].copy_from_slice(&[0x00, 0, 0, 0, 0x0C, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0, 0]); // size 0
    let t = read_table(&mut empty).unwrap();
    assert_eq!((t.scheme, t.partitions), (Scheme::Mbr { disk_signature: 0 }, vec![]));

    // Entries that run past the end of a 64-sector disk are skipped; one that ends exactly at it is kept.
    let mut small = Disk { sector: 512, data: vec![0; 512 * 64], fail: None, reads: 0 };
    small.data[510] = 0x55;
    small.data[511] = 0xAA;
    small.data[446..462].copy_from_slice(&[0x00, 0, 0, 0, 0x0C, 0, 0, 0, 0x20, 0, 0, 0, 0x21, 0, 0, 0]); // 32 + 33 > 64
    small.data[462..478].copy_from_slice(&[0x00, 0, 0, 0, 0x0C, 0, 0, 0, 0x40, 0, 0, 0, 0x01, 0, 0, 0]); // starts at 64
    small.data[478..494].copy_from_slice(&[0x00, 0, 0, 0, 0x83, 0, 0, 0, 0x20, 0, 0, 0, 0x20, 0, 0, 0]); // 32..=63
    let t = read_table(&mut small).unwrap();
    assert_eq!(t.partitions, [mbr_part(3, 32, 32, 0x83, false)]);
}

#[test]
fn errors_and_sector_sizes() {
    let mut disk = gpt_disk(512, 16_384, &standard_parts(512, 16_384));
    disk.fail = Some(0);
    assert_eq!(read_table(&mut disk), Err(PartError::Io));
    disk.fail = Some(1);
    assert_eq!(read_table(&mut disk), Err(PartError::Io), "read errors are never masked by the backup");
    for sector in [256usize, 1000, 8192] {
        let mut odd = Disk { sector, data: vec![0; sector * 64], fail: None, reads: 0 };
        assert_eq!(read_table(&mut odd), Err(PartError::Unsupported), "{sector}-byte sectors");
    }
}

#[test]
fn random_damage_never_panics_or_lies() {
    let base = gpt_disk(512, 4096, &standard_parts(512, 4096));
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let hot: Vec<usize> = (0..3 * 512).chain(base.data.len() - 33 * 512..base.data.len()).collect();
    for _ in 0..3000 {
        let mut disk = Disk { sector: 512, data: base.data.clone(), fail: None, reads: 0 };
        for _ in 0..1 + next() % 6 {
            let i = hot[(next() as usize) % hot.len()];
            disk.data[i] = next() as u8;
        }
        if let Ok(t) = read_table(&mut disk) {
            for p in &t.partitions {
                assert!(p.sectors >= 1 && p.first_lba + p.sectors <= 4096, "{p:?} lies outside the disk");
            }
        }
    }
}
