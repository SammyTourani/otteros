//! virtio-blk acceptance tests for brief M3-T1b, written by the orchestrator: the driver must pass
//! these unchanged. The test disk (scripts/mkdisk.py, attached by scripts/qemu.py) has 131,079
//! sectors (64 MiB plus 7, so a hard-coded capacity cannot pass), and sector n starts with ASCII
//! "OTTERDISK n" followed by the bytes (n + i) & 0xff. Tests that write restore what they change,
//! because build/data.img persists between runs.

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use otteros_kernel::drivers::virtio::blk::{self, BlkError};
use otteros_kernel::{kprintln, sched, time};

const SECTORS: u64 = 131_079;

fn expected_sector(lba: u64) -> [u8; 512] {
    let mut s = [0u8; 512];
    let header = format!("OTTERDISK {lba}");
    s[..header.len()].copy_from_slice(header.as_bytes());
    for (i, b) in s[header.len()..].iter_mut().enumerate() {
        *b = ((lba as usize + i) & 0xff) as u8;
    }
    s
}

fn assert_sectors(first: u64, data: &[u8]) {
    for (k, chunk) in data.chunks(512).enumerate() {
        let lba = first + k as u64;
        assert!(chunk == expected_sector(lba), "sector {lba} does not hold its pattern");
    }
}

fn vda() -> &'static blk::VirtioBlk {
    blk::vda().expect("virtio-blk vda was probed at boot")
}

#[test_case]
fn virtio_blk_capacity_comes_from_the_device() {
    assert_eq!(vda().capacity(), SECTORS, "capacity read from the device configuration");
}

#[test_case]
fn virtio_blk_reads_known_sectors() {
    let mut buf = [0u8; 512];
    for lba in [0, 1, 7, 511, 512, 4096, 65_535, 65_536, SECTORS - 1] {
        vda().read(lba, &mut buf).expect("read one sector");
        assert_sectors(lba, &buf);
    }
}

#[test_case]
fn virtio_blk_reads_many_sectors_in_one_request() {
    let dev = vda();
    let mut buf = vec![0u8; 256 * 1024];
    let before = dev.requests();
    dev.read(1000, &mut buf).expect("256 KiB read");
    assert_eq!(dev.requests() - before, 1, "a 256 KiB read is one device request");
    assert_sectors(1000, &buf);
    // The last three sectors of the disk, and a read spanning more than one request.
    let mut tail = vec![0u8; 3 * 512];
    dev.read(SECTORS - 3, &mut tail).expect("tail read");
    assert_sectors(SECTORS - 3, &tail);
    let mut big = vec![0u8; 1024 * 1024 + 512];
    dev.read(20_000, &mut big).expect("1 MiB + 1 sector read");
    assert_sectors(20_000, &big);
}

#[test_case]
fn virtio_blk_rejects_bad_requests() {
    let dev = vda();
    let mut odd = [0u8; 100];
    assert_eq!(dev.read(0, &mut odd), Err(BlkError::InvalidLength));
    assert_eq!(dev.read(0, &mut []), Err(BlkError::InvalidLength));
    let mut one = [0u8; 512];
    assert_eq!(dev.read(SECTORS, &mut one), Err(BlkError::OutOfRange));
    let mut two = [0u8; 1024];
    assert_eq!(dev.read(SECTORS - 1, &mut two), Err(BlkError::OutOfRange));
    assert_eq!(dev.write(u64::MAX, &one), Err(BlkError::OutOfRange));
    // Nothing was written by the rejected requests.
    dev.read(SECTORS - 1, &mut one).unwrap();
    assert_sectors(SECTORS - 1, &one);
}

#[test_case]
fn virtio_blk_write_read_back_and_restore() {
    let dev = vda();
    let first = 130_000u64;
    let n = 16usize;
    let mut original = vec![0u8; n * 512];
    dev.read(first, &mut original).unwrap();
    assert_sectors(first, &original);
    let mut new = vec![0u8; n * 512];
    for (i, b) in new.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(31) ^ 0x5a;
    }
    dev.write(first, &new).expect("write 16 sectors");
    dev.flush().expect("flush");
    let mut back = vec![0u8; n * 512];
    dev.read(first, &mut back).unwrap();
    assert!(back == new, "written sectors read back");
    let mut neighbour = [0u8; 512];
    dev.read(first + n as u64, &mut neighbour).unwrap();
    assert_sectors(first + n as u64, &neighbour);
    dev.write(first, &original).expect("restore");
    dev.flush().expect("flush");
    dev.read(first, &mut back).unwrap();
    assert_sectors(first, &back);
}

#[test_case]
fn virtio_blk_sequential_throughput() {
    let dev = vda();
    let mut buf = vec![0u8; 256 * 1024];
    let start = time::uptime_ms();
    let mut lba = 0u64;
    while lba < 16_384 {
        dev.read(lba, &mut buf).unwrap();
        assert_sectors(lba, &buf[..512]);
        lba += (buf.len() / 512) as u64;
    }
    let ms = (time::uptime_ms() - start).max(1);
    kprintln!("[virtio-blk] seq read 8 MiB in {} ms = {} MiB/s", ms, 8 * 1000 / ms);
}

/// Two readers and a writer run at once (preemptively scheduled): a request's header, data and
/// status must never be mixed with another request's, so every read returns exactly its sectors.
#[test_case]
fn virtio_blk_concurrent_requests_do_not_mix() {
    fn reader(seed: usize) -> i32 {
        let dev = vda();
        let mut x = seed as u64 * 0x9e37_79b9 + 1;
        let mut buf = vec![0u8; 64 * 512];
        for _ in 0..400 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let n = 1 + (x % 64) as usize;
            let lba = (x >> 8) % 100_000;
            let b = &mut buf[..n * 512];
            dev.read(lba, b).expect("concurrent read");
            for (k, chunk) in b.chunks(512).enumerate() {
                let sector = lba + k as u64;
                assert!(chunk == expected_sector(sector), "reader {seed}: sector {sector} came back wrong");
            }
        }
        0
    }
    fn writer(_: usize) -> i32 {
        let dev = vda();
        let (first, n) = (125_000u64, 32usize);
        let mut original = vec![0u8; n * 512];
        dev.read(first, &mut original).expect("read the scratch area");
        let mut back = vec![0u8; n * 512];
        for round in 0..200u32 {
            let data: Vec<u8> = (0..n * 512).map(|i| (i as u32 ^ round.wrapping_mul(2_654_435_761)) as u8).collect();
            dev.write(first, &data).expect("concurrent write");
            dev.read(first, &mut back).expect("read back");
            assert!(back == data, "writer: round {round} read back different data");
        }
        dev.write(first, &original).expect("restore");
        0
    }
    let a = sched::spawn("vblk-reader-a", reader, 1);
    let b = sched::spawn("vblk-reader-b", reader, 2);
    let w = sched::spawn("vblk-writer", writer, 0);
    assert_eq!(sched::join(a), 0);
    assert_eq!(sched::join(b), 0);
    assert_eq!(sched::join(w), 0);
    let mut check = vec![0u8; 32 * 512];
    vda().read(125_000, &mut check).unwrap();
    assert_sectors(125_000, &check);
}
