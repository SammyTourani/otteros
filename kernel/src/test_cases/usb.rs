//! USB (xHCI) tests for brief M10-T1, written by the orchestrator: the implementation must pass
//! these unchanged. They run in `gmake usb-test`: QEMU q35 with `qemu-xhci`, a `usb-kbd` and a
//! `usb-storage` stick (build/usbstick.img: 64 MiB, MBR, one FAT32 partition from LBA 2048 made
//! with mtools, holding HELLO.TXT), kernel cmdline `test testfilter=usb`. The laptop boots from a
//! USB stick and keeps its data partition there, and its keyboard may be USB: both paths are
//! exercised here with QEMU's models.
//!
//! API: `drivers::usb::controllers() -> usize`; `drivers::usb::devices() -> Vec<UsbDevice>` with
//! `UsbDevice { port: u8, speed: otter_xhci::context::Speed, address: u8, vendor_id: u16,
//! product_id: u16, class: u8, subclass: u8, protocol: u8 }` (class triple of the interface the
//! kernel bound); `drivers::usb::storage::disks() -> Vec<Arc<UsbDisk>>` where `UsbDisk` has
//! `inquiry() -> otter_usb::msc::Inquiry`, `block_count() -> u64`, `block_size() -> usize` and
//! `read_blocks(lba, &mut [u8])` / `write_blocks(lba, &[u8]) -> Result<(), BlockError>` (the block
//! layer's device interface). USB keyboards feed the same decoder as PS/2:
//! `drivers::ps2::keyboard::poll_event()`.

use alloc::vec;
use otteros_kernel::drivers::ps2::keyboard;
use otteros_kernel::drivers::usb::{self, storage};
use otteros_kernel::{kprintln, time};
use otter_part::{Kind, SectorRead, read_table};

#[test_case]
fn usb_controller_enumerates_the_devices() {
    assert_eq!(usb::controllers(), 1, "one qemu-xhci");
    let devices = usb::devices();
    let kbd = devices.iter().find(|d| (d.class, d.subclass, d.protocol) == (3, 1, 1)).expect("a boot keyboard");
    assert_eq!((kbd.vendor_id, kbd.product_id), (0x0627, 0x0001), "QEMU's usb-kbd");
    let stick = devices.iter().find(|d| (d.class, d.subclass, d.protocol) == (8, 6, 0x50)).expect("a bulk-only SCSI stick");
    assert_eq!(stick.vendor_id, 0x46F4, "QEMU's usb-storage");
    let mut addresses: vec::Vec<u8> = devices.iter().map(|d| d.address).collect();
    addresses.sort_unstable();
    addresses.dedup();
    assert_eq!(addresses.len(), devices.len(), "every device got its own address");
}

#[test_case]
fn usb_keyboard_types_through_the_shared_decoder() {
    kprintln!("[usbkbd] ready");
    const EXPECTED: &str = "hello";
    let mut received = alloc::string::String::new();
    let deadline = time::uptime_ms() + 10_000;
    while received.len() < EXPECTED.len() {
        if let Some(ch) = keyboard::poll_event().and_then(|e| e.to_char()) {
            received.push(ch);
            continue;
        }
        assert!(time::uptime_ms() < deadline, "timed out, got {received:?} so far");
        time::sleep_ms(1);
    }
    assert_eq!(received, EXPECTED);
}

struct Sectors(alloc::sync::Arc<storage::UsbDisk>);

impl SectorRead for Sectors {
    fn sector_size(&self) -> usize {
        self.0.block_size()
    }
    fn sector_count(&self) -> u64 {
        self.0.block_count()
    }
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), otter_part::ReadError> {
        self.0.read_blocks(lba, buf).map_err(|_| otter_part::ReadError)
    }
}

#[test_case]
fn usb_stick_identifies_and_holds_a_fat32_partition() {
    let disks = storage::disks();
    assert_eq!(disks.len(), 1);
    let disk = disks[0].clone();
    let inquiry = disk.inquiry();
    assert_eq!((inquiry.vendor.as_str(), inquiry.peripheral_type), ("QEMU", 0));
    assert_eq!((disk.block_size(), disk.block_count()), (512, 131_072), "64 MiB");
    let mut mbr = vec![0u8; 512];
    disk.read_blocks(0, &mut mbr).expect("read LBA 0");
    assert_eq!(&mbr[510..], &[0x55, 0xAA]);
    let table = read_table(&mut Sectors(disk.clone())).expect("a partition table");
    let fat = table.partitions.iter().find(|p| matches!(p.kind, Kind::Mbr { system_id: 0x0B | 0x0C })).expect("a FAT32 partition");
    assert_eq!(fat.first_lba, 2048);
    let mut boot = vec![0u8; 512];
    disk.read_blocks(fat.first_lba, &mut boot).unwrap();
    assert_eq!(&boot[82..90], b"FAT32   ", "the partition's boot sector");
}

#[test_case]
fn usb_stick_multi_block_io_matches_single_blocks() {
    let disk = storage::disks()[0].clone();
    let mut run = vec![0u8; 64 * 512];
    disk.read_blocks(2048, &mut run).expect("a 32 KiB read");
    for i in 0..64u64 {
        let mut one = vec![0u8; 512];
        disk.read_blocks(2048 + i, &mut one).unwrap();
        assert!(one[..] == run[i as usize * 512..(i as usize + 1) * 512], "block {i}");
    }
    // Write a pattern to the last 8 blocks (outside the partition), read it back, twice.
    let last = disk.block_count() - 8;
    for round in 0..2u8 {
        let pattern: vec::Vec<u8> = (0..8 * 512).map(|i| (i as u8).wrapping_mul(31).wrapping_add(round)).collect();
        disk.write_blocks(last, &pattern).expect("write");
        let mut back = vec![0u8; 8 * 512];
        disk.read_blocks(last, &mut back).expect("read back");
        assert!(back == pattern, "round {round}");
    }
    let mut odd = vec![0u8; 100];
    assert!(disk.read_blocks(0, &mut odd).is_err(), "buffers must be whole blocks");
    let mut beyond = vec![0u8; 512];
    assert!(disk.read_blocks(disk.block_count(), &mut beyond).is_err(), "past the end");
}
