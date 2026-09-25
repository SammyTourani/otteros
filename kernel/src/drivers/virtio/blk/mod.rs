//! Virtio block device driver (brief M3-T1b): polled read/write on virtio-blk via otter-virtio.

mod pci;

use crate::acpi;
use crate::drivers::pci::{find_device, ecam};
use crate::kprintln;
use crate::mm::addr::{PhysAddr, VirtAddr, FRAME_SIZE};
use crate::mm::{mmio, pmm};
use crate::sched;
use crate::sync::IrqMutex;
use crate::time;
use alloc::boxed::Box;
use otter_virtio::{
    negotiate, queue_size_for, read_config_consistent, setup_queue, finish_init, BlockRequest,
    Buffer, Layout, RingMemory, SplitQueue, Transport, BLOCK_T_IN, BLOCK_T_OUT, BLOCK_T_FLUSH,
    BLOCK_S_OK, VIRTIO_F_RING_EVENT_IDX,
};
use pci::{VirtioTransport, read_pci_u8, read_pci_u32, write_pci_u16};

const VIRTIO_VENDOR: u16 = 0x1af4;
const VIRTIO_BLK_DEVICE: u16 = 0x1042;
const VIRTIO_BLK_F_FLUSH: u64 = 1u64 << 9;

/// Error type for block device operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlkError {
    InvalidLength,
    OutOfRange,
    Io(u8),
    Timeout,
}


/// The virtio-blk device state.
pub struct VirtioBlk {
    transport: VirtioTransport,
    queue: SplitQueue,
    capacity: u64,
    request_count: u64,
    bounce_buf_phys: PhysAddr,
    bounce_buf_virt: VirtAddr,
    features: u64,
    request_mutex: sched::Mutex<()>,
}

// SAFETY: All pointers in VirtioBlk refer to memory we own exclusively.
unsafe impl Send for VirtioBlk {}
unsafe impl Sync for VirtioBlk {}

/// Global device wrapper that provides exclusive access.
pub struct VirtioBlkDevice {
    device: Option<Box<VirtioBlk>>,
}

/// Global device instance (protected by IrqMutex for interrupt-safe access).
static DEVICE: IrqMutex<VirtioBlkDevice> = IrqMutex::new(VirtioBlkDevice { device: None });


/// Initialize the virtio-blk driver.
pub fn init() {
    let pci_dev = match find_device(VIRTIO_VENDOR, VIRTIO_BLK_DEVICE) {
        Some(d) => {
            kprintln!("[virtio-blk] found device");
            d
        }
        None => {
            kprintln!("[virtio-blk] device not found");
            return;
        }
    };

    // Get ECAM base address from ACPI MCFG table
    let ecam_base_addr = acpi::with_info(|info| {
        info.mcfg.as_ref().and_then(|mcfg| {
            mcfg.allocations.iter().find(|alloc| {
                pci_dev.bdf.bus >= alloc.bus_start && pci_dev.bdf.bus <= alloc.bus_end
            }).map(|alloc| alloc.base_addr)
        })
    });

    let ecam_base_addr = match ecam_base_addr {
        Some(addr) => addr,
        None => {
            kprintln!("[virtio-blk] MCFG allocation not found for device bus {}", pci_dev.bdf.bus);
            return;
        }
    };

    // Enable Memory Space (bit 1) and Bus Master (bit 2) in PCI command register
    let ecam_base = ecam::map_ecam_window(ecam_base_addr, 0x1000);
    // Compute ECAM offset for this device (bus:device.function)
    let ecam_offset = ((pci_dev.bdf.bus as u32) << 20)
        | ((pci_dev.bdf.device as u32) << 15)
        | ((pci_dev.bdf.function as u32) << 12);

    // SAFETY: offset 0x04 is the command register in PCI config space
    let cmd = unsafe { ecam::read_u16(ecam_base, ecam_offset + 0x04) };
    let cmd_new = cmd | 0x0006; // Enable Memory Space (bit 1) and Bus Master (bit 2)
    write_pci_u16(ecam_base, ecam_offset, 0x04, cmd_new);

    // Parse virtio PCI capability structures
    let mut common_cfg: Option<(usize, u32, u32)> = None;  // (bar_idx, offset, length)
    let mut notify_cfg: Option<(usize, u32, u32, u32)> = None;  // (bar_idx, offset, length, multiplier)
    let mut device_cfg: Option<(usize, u32, u32)> = None;  // (bar_idx, offset, length)

    for &(cap_id, cap_offset) in &pci_dev.capabilities {
        if cap_id != 0x09 {
            continue;  // Not a vendor-specific capability
        }

        // Parse the virtio capability at this offset
        // Structure: +0: cap_id, +1: next, +2: length, +3: cfg_type, +4: bar, +8: offset, +12: length, +16: notify_off_multiplier
        let cfg_type = read_pci_u8(ecam_base, ecam_offset, cap_offset, 3);
        let bar = read_pci_u8(ecam_base, ecam_offset, cap_offset, 4) as usize;
        let offset = read_pci_u32(ecam_base, ecam_offset, cap_offset, 8);
        let length = read_pci_u32(ecam_base, ecam_offset, cap_offset, 12);

        kprintln!("[virtio-blk] capability type {} at bar {} offset 0x{:x} length {}", cfg_type, bar, offset, length);

        match cfg_type {
            1 => {
                // Common configuration
                common_cfg = Some((bar, offset, length));
            }
            2 => {
                // Notify configuration
                let multiplier = read_pci_u32(ecam_base, ecam_offset, cap_offset, 16);
                notify_cfg = Some((bar, offset, length, multiplier));
            }
            3 => {
                // ISR configuration (not used for now)
            }
            4 => {
                // Device configuration
                device_cfg = Some((bar, offset, length));
            }
            _ => {}
        }
    }

    let (common_bar_idx, common_offset, common_len) = match common_cfg {
        Some((bar, off, len)) => (bar, off, len as usize),
        None => {
            kprintln!("[virtio-blk] could not find common config capability");
            return;
        }
    };

    let (notify_bar_idx, notify_offset, notify_len, notify_mul) = match notify_cfg {
        Some((bar, off, len, mul)) => (bar, off, len as usize, mul),
        None => {
            kprintln!("[virtio-blk] could not find notify capability");
            return;
        }
    };

    let (device_bar_idx, device_offset, device_len) = match device_cfg {
        Some((bar, off, len)) => (bar, off, len as usize),
        None => {
            kprintln!("[virtio-blk] could not find device config capability");
            return;
        }
    };

    // Get physical addresses
    let common_phys_base = match pci_dev.bars[common_bar_idx] {
        Some(a) => a,
        None => {
            kprintln!("[virtio-blk] common config BAR is None");
            return;
        }
    };

    let notify_phys_base = match pci_dev.bars[notify_bar_idx] {
        Some(a) => a,
        None => {
            kprintln!("[virtio-blk] notify BAR is None");
            return;
        }
    };

    let common_virt = match mmio::map(PhysAddr::new(common_phys_base + common_offset as u64), common_len) {
        Some(a) => {
            kprintln!("[virtio-blk] common bar{} offset 0x{:x}", common_bar_idx, common_offset);
            a
        }
        None => {
            kprintln!("[virtio-blk] failed to map common config");
            return;
        }
    };

    let notify_virt = match mmio::map(PhysAddr::new(notify_phys_base + notify_offset as u64), notify_len) {
        Some(a) => {
            kprintln!("[virtio-blk] notify bar{} offset 0x{:x} x{}", notify_bar_idx, notify_offset, notify_mul);
            a
        }
        None => {
            kprintln!("[virtio-blk] failed to map notify");
            return;
        }
    };

    let device_phys_base = match pci_dev.bars[device_bar_idx] {
        Some(a) => a,
        None => {
            kprintln!("[virtio-blk] device config BAR is None");
            return;
        }
    };

    let device_virt = match mmio::map(PhysAddr::new(device_phys_base + device_offset as u64), device_len) {
        Some(a) => {
            kprintln!("[virtio-blk] device bar{} offset 0x{:x}", device_bar_idx, device_offset);
            a
        }
        None => {
            kprintln!("[virtio-blk] failed to map device config");
            return;
        }
    };

    let transport = VirtioTransport::new(common_virt, notify_virt, device_virt, notify_mul);

    // Negotiate features: VERSION_1, EVENT_IDX, and FLUSH if offered
    let negotiated = match negotiate(&transport, VIRTIO_F_RING_EVENT_IDX | VIRTIO_BLK_F_FLUSH) {
        Ok(f) => {
            kprintln!("[virtio-blk] negotiated features: 0x{:x}", f);
            f
        }
        Err(e) => {
            kprintln!("[virtio-blk] feature negotiation failed: {:?}", e);
            return;
        }
    };

    // Setup queue
    let queue_size = match queue_size_for(&transport, 0, 256) {
        Ok(s) => {
            kprintln!("[virtio-blk] queue size: {}", s);
            s
        }
        Err(e) => {
            kprintln!("[virtio-blk] queue size query failed: {:?}", e);
            return;
        }
    };

    let layout = match Layout::for_size(queue_size) {
        Some(l) => {
            kprintln!("[virtio-blk] layout total_size: {}", l.total_size);
            l
        }
        None => {
            kprintln!("[virtio-blk] invalid queue size");
            return;
        }
    };

    let ring_frames = layout.total_size.div_ceil(FRAME_SIZE);
    let ring_phys = match pmm::alloc_contiguous(ring_frames, 1) {
        Some(a) => {
            kprintln!("[virtio-blk] allocated ring memory");
            a
        }
        None => {
            kprintln!("[virtio-blk] failed to allocate ring memory");
            return;
        }
    };

    let ring_virt = crate::mm::hhdm::phys_to_virt(ring_phys);
    let ring_mem = match unsafe {
        // SAFETY: ring_virt is a valid HHDM mapping of ring_phys, which is contiguous memory
        // allocated by pmm::alloc_contiguous. The pointers are valid for the entire layout.total_size.
        RingMemory::new(ring_virt.as_u64() as *mut u8, ring_phys.as_u64(), layout.total_size)
    } {
        Some(m) => {
            kprintln!("[virtio-blk] created ring memory");
            m
        }
        None => {
            kprintln!("[virtio-blk] failed to create ring memory");
            return;
        }
    };

    let queue = match SplitQueue::new(layout, ring_mem) {
        Some(q) => {
            kprintln!("[virtio-blk] created split queue");
            q
        }
        None => {
            kprintln!("[virtio-blk] failed to create split queue");
            return;
        }
    };

    if setup_queue(&transport, 0, &queue, 0).is_err() {
        kprintln!("[virtio-blk] failed to setup queue");
        return;
    }
    kprintln!("[virtio-blk] setup queue");

    finish_init(&transport);
    kprintln!("[virtio-blk] finished init");

    // Read capacity
    let mut cap_buf = [0u8; 8];
    read_config_consistent(&transport, 0, &mut cap_buf);
    let capacity = u64::from_le_bytes(cap_buf);
    kprintln!("[virtio-blk] read capacity: {}", capacity);

    // Allocate bounce buffer
    let bounce_size: usize = 256 * 1024 + 16 + 1;
    let bounce_frames = bounce_size.div_ceil(FRAME_SIZE);
    let bounce_phys = match pmm::alloc_contiguous(bounce_frames, 1) {
        Some(a) => a,
        None => return,
    };

    let bounce_virt = crate::mm::hhdm::phys_to_virt(bounce_phys);

    let dev = VirtioBlk {
        transport,
        queue,
        capacity,
        request_count: 0,
        bounce_buf_phys: bounce_phys,
        bounce_buf_virt: bounce_virt,
        features: negotiated,
        request_mutex: sched::Mutex::new(()),
    };

    let mib = capacity / 2048;
    kprintln!("[virtio-blk] vda: {} sectors ({} MiB), features 0x{:x}", capacity, mib, negotiated);

    let mut guard = DEVICE.lock();
    *guard = VirtioBlkDevice { device: Some(Box::new(dev)) };
    kprintln!("[virtio-blk] device stored in global");
}

/// Get a reference to the virtio-blk device.
pub fn vda() -> Option<&'static VirtioBlk> {
    // SAFETY: The device is initialized once at boot and never moved or freed.
    // All access to the mutable state (queue, request_count) goes through the IrqMutex lock in the methods,
    // which prevents concurrent modification even across interrupt handlers. Returning a 'static reference
    // is safe because the device lives for the entire kernel lifetime and reads its capacity from MMIO
    // at initialization (never modified after).
    unsafe {
        let device_ref = &*(&DEVICE as *const IrqMutex<VirtioBlkDevice>);
        let guard = device_ref.lock();
        guard.device.as_ref().map(|b| {
            let b_ref = b.as_ref();
            core::mem::transmute::<&VirtioBlk, &'static VirtioBlk>(b_ref)
        })
    }
}

impl VirtioBlk {
    /// Returns the device capacity in sectors.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Returns the number of completed requests.
    pub fn requests(&self) -> u64 {
        let guard = DEVICE.lock();
        guard.device.as_ref().map(|b| b.request_count).unwrap_or(0)
    }

    /// Reads sectors from the device.
    pub fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlkError> {
        if buf.is_empty() || !buf.len().is_multiple_of(512) {
            return Err(BlkError::InvalidLength);
        }

        let num_sectors = (buf.len() / 512) as u64;
        if lba.checked_add(num_sectors).ok_or(BlkError::OutOfRange)? > self.capacity {
            return Err(BlkError::OutOfRange);
        }

        // Hold the request mutex for the entire operation to prevent other threads from
        // interleaving requests. This ensures only one request is in the device queue at a time.
        let _req_guard = self.request_mutex.lock();

        let max_per_req = 256 * 1024 / 512;
        let mut sector = lba;
        let mut offset = 0;

        while offset < buf.len() {
            let left = (buf.len() - offset) / 512;
            let this = ((left as u64).min(max_per_req) * 512) as usize;

            let req = BlockRequest::new(BLOCK_T_IN, sector);
            let req_bytes = req.to_bytes();

            // SAFETY: bounce_buf_virt is a valid HHDM pointer to the bounce buffer, which is at
            // least 256 KiB + 16 + 1 bytes. We're copying 16 bytes to the beginning of the buffer.
            // The request_mutex ensures no other thread accesses the bounce buffer concurrently.
            unsafe {
                core::ptr::copy_nonoverlapping(req_bytes.as_ptr(), self.bounce_buf_virt.as_u64() as *mut u8, 16);
            }

            let mut guard = DEVICE.lock();
            let dev = guard.device.as_mut().ok_or(BlkError::Io(99))?;

            let buffers = [
                Buffer { addr: dev.bounce_buf_phys.as_u64(), len: 16, device_writes: false },
                Buffer { addr: dev.bounce_buf_phys.as_u64() + 16, len: this as u32, device_writes: true },
                Buffer { addr: dev.bounce_buf_phys.as_u64() + 16 + this as u64, len: 1, device_writes: true },
            ];

            dev.queue.add(&buffers, (sector << 16) | (this as u64 / 512)).map_err(|_| BlkError::Io(99))?;
            if dev.queue.should_notify() {
                dev.transport.notify(0, 0);
            }
            dev.request_count += 1;

            drop(guard);

            // Poll for completion (while still holding request_mutex for serialization)
            let deadline = time::uptime_ms() + 5000;
            loop {
                let mut guard = DEVICE.lock();
                let dev = guard.device.as_mut().ok_or(BlkError::Io(99))?;

                if let Some((_, _)) = dev.queue.pop_used() {
                    // SAFETY: bounce_buf_virt + 16 + this is within the bounce buffer's allocated range.
                    // The request_mutex ensures no concurrent access to the bounce buffer.
                    let status = unsafe { core::ptr::read_volatile((dev.bounce_buf_virt.as_u64() + 16 + this as u64) as *const u8) };
                    if status != BLOCK_S_OK as u8 {
                        drop(guard);
                        drop(_req_guard);
                        return Err(BlkError::Io(status));
                    }
                    // SAFETY: bounce_buf_virt + 16 contains the data region of the bounce buffer (size `this`),
                    // and buf.as_mut_ptr().add(offset) points to a valid mutable slice of size at least `this`.
                    // The request_mutex ensures exclusive access to the bounce buffer.
                    unsafe {
                        core::ptr::copy_nonoverlapping((dev.bounce_buf_virt.as_u64() + 16) as *const u8, buf.as_mut_ptr().add(offset), this);
                    }
                    drop(guard);
                    break;
                }
                drop(guard);

                if time::uptime_ms() >= deadline {
                    drop(_req_guard);
                    return Err(BlkError::Timeout);
                }
                core::hint::spin_loop();
            }

            // _req_guard is dropped here, releasing the mutex for the next request

            sector += (this / 512) as u64;
            offset += this;
        }

        Ok(())
    }

    /// Writes sectors to the device.
    pub fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlkError> {
        if buf.is_empty() || !buf.len().is_multiple_of(512) {
            return Err(BlkError::InvalidLength);
        }

        let num_sectors = (buf.len() / 512) as u64;
        if lba.checked_add(num_sectors).ok_or(BlkError::OutOfRange)? > self.capacity {
            return Err(BlkError::OutOfRange);
        }

        // Hold the request mutex for the entire operation to prevent other threads from
        // interleaving requests. This ensures only one request is in the device queue at a time.
        let _req_guard = self.request_mutex.lock();

        let max_per_req = 256 * 1024 / 512;
        let mut sector = lba;
        let mut offset = 0;

        while offset < buf.len() {
            let left = (buf.len() - offset) / 512;
            let this = ((left as u64).min(max_per_req) * 512) as usize;

            let req = BlockRequest::new(BLOCK_T_OUT, sector);
            let req_bytes = req.to_bytes();

            // SAFETY: bounce_buf_virt is a valid HHDM pointer. We're copying the request header
            // to the first 16 bytes and data to offset 16, within the allocated bounce buffer.
            // The request_mutex ensures no other thread accesses the bounce buffer concurrently.
            unsafe {
                core::ptr::copy_nonoverlapping(req_bytes.as_ptr(), self.bounce_buf_virt.as_u64() as *mut u8, 16);
                core::ptr::copy_nonoverlapping(buf.as_ptr().add(offset), (self.bounce_buf_virt.as_u64() + 16) as *mut u8, this);
            }

            let mut guard = DEVICE.lock();
            let dev = guard.device.as_mut().ok_or(BlkError::Io(99))?;

            let buffers = [
                Buffer { addr: dev.bounce_buf_phys.as_u64(), len: 16, device_writes: false },
                Buffer { addr: dev.bounce_buf_phys.as_u64() + 16, len: this as u32, device_writes: false },
                Buffer { addr: dev.bounce_buf_phys.as_u64() + 16 + this as u64, len: 1, device_writes: true },
            ];

            dev.queue.add(&buffers, (sector << 16) | (this as u64 / 512)).map_err(|_| BlkError::Io(99))?;
            if dev.queue.should_notify() {
                dev.transport.notify(0, 0);
            }
            dev.request_count += 1;

            drop(guard);

            // Poll for completion (while still holding request_mutex for serialization)
            let deadline = time::uptime_ms() + 5000;
            loop {
                let mut guard = DEVICE.lock();
                let dev = guard.device.as_mut().ok_or(BlkError::Io(99))?;

                if let Some((_, _)) = dev.queue.pop_used() {
                    // SAFETY: bounce_buf_virt + 16 + this is the status byte location within the bounce buffer.
                    // The request_mutex ensures no concurrent access to the bounce buffer.
                    let status = unsafe { core::ptr::read_volatile((dev.bounce_buf_virt.as_u64() + 16 + this as u64) as *const u8) };
                    if status != BLOCK_S_OK as u8 {
                        drop(guard);
                        drop(_req_guard);
                        return Err(BlkError::Io(status));
                    }
                    drop(guard);
                    break;
                }
                drop(guard);

                if time::uptime_ms() >= deadline {
                    drop(_req_guard);
                    return Err(BlkError::Timeout);
                }
                core::hint::spin_loop();
            }

            // _req_guard is dropped here, releasing the mutex for the next request

            sector += (this / 512) as u64;
            offset += this;
        }

        Ok(())
    }

    /// Flushes the device. Returns Ok without doing anything if FLUSH was not negotiated.
    pub fn flush(&self) -> Result<(), BlkError> {
        // Only send flush if the device supports it
        if (self.features & VIRTIO_BLK_F_FLUSH) == 0 {
            return Ok(());
        }

        // Hold the request mutex for the entire operation: copy-in, submit, poll
        let _req_guard = self.request_mutex.lock();

        let req = BlockRequest::new(BLOCK_T_FLUSH, 0);
        let req_bytes = req.to_bytes();

        // SAFETY: bounce_buf_virt is a valid HHDM pointer to the bounce buffer.
        // We're copying 16 bytes to the beginning of the buffer. The request_mutex
        // ensures no other thread accesses the bounce buffer concurrently.
        unsafe {
            core::ptr::copy_nonoverlapping(req_bytes.as_ptr(), self.bounce_buf_virt.as_u64() as *mut u8, 16);
        }

        let mut guard = DEVICE.lock();
        let dev = guard.device.as_mut().ok_or(BlkError::Io(99))?;

        let buffers = [
            Buffer { addr: dev.bounce_buf_phys.as_u64(), len: 16, device_writes: false },
            Buffer { addr: dev.bounce_buf_phys.as_u64() + 16, len: 1, device_writes: true },
        ];

        dev.queue.add(&buffers, 0).map_err(|_| BlkError::Io(99))?;
        if dev.queue.should_notify() {
            dev.transport.notify(0, 0);
        }
        dev.request_count += 1;

        drop(guard);

        // Poll for completion (while still holding request_mutex for serialization)
        let deadline = time::uptime_ms() + 5000;
        loop {
            let mut guard = DEVICE.lock();
            let dev = guard.device.as_mut().ok_or(BlkError::Io(99))?;

            if let Some((_, _)) = dev.queue.pop_used() {
                // SAFETY: bounce_buf_virt + 16 is the status byte location for the flush request.
                // The request_mutex ensures no concurrent access to the bounce buffer.
                let status = unsafe { core::ptr::read_volatile((dev.bounce_buf_virt.as_u64() + 16) as *const u8) };
                if status != BLOCK_S_OK as u8 {
                    drop(guard);
                    drop(_req_guard);
                    return Err(BlkError::Io(status));
                }
                drop(guard);
                break;
            }
            drop(guard);

            if time::uptime_ms() >= deadline {
                drop(_req_guard);
                return Err(BlkError::Timeout);
            }
            core::hint::spin_loop();
        }

        // _req_guard is dropped here, releasing the mutex

        Ok(())
    }
}
