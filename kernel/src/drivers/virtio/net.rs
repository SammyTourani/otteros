//! Virtio network device driver (brief M5-T1a): Ethernet RX/TX on virtio-net via otter-virtio.

use crate::acpi;
use crate::drivers::pci::{find_device, ecam};
use crate::drivers::virtio::pci::{VirtioTransport, read_pci_u8, read_pci_u32, write_pci_u16};
use crate::kprintln;
use crate::mm::addr::{PhysAddr, VirtAddr, FRAME_SIZE};
use crate::mm::{mmio, pmm};
use crate::sync::IrqMutex;
use crate::time;
use alloc::boxed::Box;
use otter_virtio::{
    negotiate, queue_size_for, read_config_consistent, setup_queue, finish_init,
    Buffer, Layout, RingMemory, SplitQueue, Transport,
};

const VIRTIO_VENDOR: u16 = 0x1af4;
const VIRTIO_NET_DEVICE: u16 = 0x1041;
const VIRTIO_NET_F_MAC: u64 = 1u64 << 5;

/// Virtio-net frame header (12 bytes).
#[repr(C, packed)]
struct VirtioNetHeader {
    flags: u8,
    gso_type: u8,
    hdr_len: u16,
    gso_size: u16,
    csum_start: u16,
    csum_offset: u16,
    _num_buffers: u16,
}

impl VirtioNetHeader {
    fn new() -> Self {
        VirtioNetHeader {
            flags: 0,
            gso_type: 0,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
            _num_buffers: 0,
        }
    }
}

/// Error type for network operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    InvalidLength,
    OutOfRange,
    Io(u8),
    Timeout,
}

/// The virtio-net device state.
pub struct VirtioNet {
    transport: VirtioTransport,
    rx_queue: SplitQueue,
    tx_queue: SplitQueue,
    mac: [u8; 6],
    rx_buffers: [PhysAddr; 64],
    rx_virt_buffers: [VirtAddr; 64],
    tx_bounce_buf_phys: PhysAddr,
    tx_bounce_buf_virt: VirtAddr,
    #[allow(dead_code)]
    features: u64,
    rx_queue_notify_off: u16,
    tx_queue_notify_off: u16,
    rx_count: u64,
    tx_count: u64,
}

// SAFETY: All pointers in VirtioNet refer to memory we own exclusively.
unsafe impl Send for VirtioNet {}
unsafe impl Sync for VirtioNet {}

/// Global device wrapper that provides exclusive access.
pub struct VirtioNetDevice {
    device: Option<Box<VirtioNet>>,
}

/// Global device instance (protected by IrqMutex for interrupt-safe access).
static DEVICE: IrqMutex<VirtioNetDevice> = IrqMutex::new(VirtioNetDevice { device: None });

/// Initialize the virtio-net driver.
pub fn init() {
    let pci_dev = match find_device(VIRTIO_VENDOR, VIRTIO_NET_DEVICE) {
        Some(d) => {
            kprintln!("[virtio-net] found device");
            d
        }
        None => {
            kprintln!("[virtio-net] device not found");
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
            kprintln!("[virtio-net] MCFG allocation not found for device bus {}", pci_dev.bdf.bus);
            return;
        }
    };

    // Enable Memory Space (bit 1) and Bus Master (bit 2) in PCI command register
    let ecam_base = ecam::map_ecam_window(ecam_base_addr, 0x1000);
    let ecam_offset = ((pci_dev.bdf.bus as u32) << 20)
        | ((pci_dev.bdf.device as u32) << 15)
        | ((pci_dev.bdf.function as u32) << 12);

    // SAFETY: offset 0x04 is the command register in PCI config space
    let cmd = unsafe { ecam::read_u16(ecam_base, ecam_offset + 0x04) };
    let cmd_new = cmd | 0x0006; // Enable Memory Space (bit 1) and Bus Master (bit 2)
    // SAFETY: ecam_base is a valid ECAM window, and 0x04 is within PCI config space
    unsafe { write_pci_u16(ecam_base, ecam_offset, 0x04, cmd_new) };

    // Parse virtio PCI capability structures
    let mut common_cfg: Option<(usize, u32, u32)> = None;
    let mut notify_cfg: Option<(usize, u32, u32, u32)> = None;
    let mut device_cfg: Option<(usize, u32, u32)> = None;

    for &(cap_id, cap_offset) in &pci_dev.capabilities {
        if cap_id != 0x09 {
            continue;
        }

        // SAFETY: ecam_base is a valid ECAM window; cap_offset and other offsets are within PCI config space
        let cfg_type = unsafe { read_pci_u8(ecam_base, ecam_offset, cap_offset, 3) };
        let bar = unsafe { read_pci_u8(ecam_base, ecam_offset, cap_offset, 4) } as usize;
        let offset = unsafe { read_pci_u32(ecam_base, ecam_offset, cap_offset, 8) };
        let length = unsafe { read_pci_u32(ecam_base, ecam_offset, cap_offset, 12) };

        match cfg_type {
            1 => common_cfg = Some((bar, offset, length)),
            2 => {
                let multiplier = unsafe { read_pci_u32(ecam_base, ecam_offset, cap_offset, 16) };
                notify_cfg = Some((bar, offset, length, multiplier));
            }
            4 => device_cfg = Some((bar, offset, length)),
            _ => {}
        }
    }

    let (common_bar_idx, common_offset, common_len) = match common_cfg {
        Some((bar, off, len)) => (bar, off, len as usize),
        None => {
            kprintln!("[virtio-net] could not find common config capability");
            return;
        }
    };

    let (notify_bar_idx, notify_offset, notify_len, notify_mul) = match notify_cfg {
        Some((bar, off, len, mul)) => (bar, off, len as usize, mul),
        None => {
            kprintln!("[virtio-net] could not find notify capability");
            return;
        }
    };

    let (device_bar_idx, device_offset, device_len) = match device_cfg {
        Some((bar, off, len)) => (bar, off, len as usize),
        None => {
            kprintln!("[virtio-net] could not find device config capability");
            return;
        }
    };

    let common_phys_base = match pci_dev.bars[common_bar_idx] {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] common config BAR is None");
            return;
        }
    };

    let notify_phys_base = match pci_dev.bars[notify_bar_idx] {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] notify BAR is None");
            return;
        }
    };

    let common_virt = match mmio::map(PhysAddr::new(common_phys_base + common_offset as u64), common_len) {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] failed to map common config");
            return;
        }
    };

    let notify_virt = match mmio::map(PhysAddr::new(notify_phys_base + notify_offset as u64), notify_len) {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] failed to map notify");
            return;
        }
    };

    let device_phys_base = match pci_dev.bars[device_bar_idx] {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] device config BAR is None");
            return;
        }
    };

    let device_virt = match mmio::map(PhysAddr::new(device_phys_base + device_offset as u64), device_len) {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] failed to map device config");
            return;
        }
    };

    let transport = VirtioTransport::new(common_virt, notify_virt, device_virt, notify_mul);

    // Read MAC address from device config (6 bytes at offset 0)
    let mut mac = [0u8; 6];
    read_config_consistent(&transport, 0, &mut mac);
    kprintln!("[virtio-net] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

    // Negotiate features: VERSION_1 + MAC only
    let negotiated = match negotiate(&transport, VIRTIO_NET_F_MAC) {
        Ok(f) => {
            kprintln!("[virtio-net] negotiated features: 0x{:x}", f);
            f
        }
        Err(e) => {
            kprintln!("[virtio-net] feature negotiation failed: {:?}", e);
            return;
        }
    };

    // Setup RX queue (queue 0)
    let rx_queue_size = match queue_size_for(&transport, 0, 64) {
        Ok(s) => {
            kprintln!("[virtio-net] RX queue size: {}", s);
            s
        }
        Err(e) => {
            kprintln!("[virtio-net] RX queue size query failed: {:?}", e);
            return;
        }
    };

    let rx_layout = match Layout::for_size(rx_queue_size) {
        Some(l) => l,
        None => {
            kprintln!("[virtio-net] invalid RX queue size");
            return;
        }
    };

    let rx_ring_frames = rx_layout.total_size.div_ceil(FRAME_SIZE);
    let rx_ring_phys = match pmm::alloc_contiguous(rx_ring_frames, 1) {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] failed to allocate RX ring memory");
            return;
        }
    };

    let rx_ring_virt = crate::mm::hhdm::phys_to_virt(rx_ring_phys);
    let rx_ring_mem = match unsafe {
        // SAFETY: ring_virt is a valid HHDM mapping of ring_phys, which is contiguous memory
        // allocated by pmm::alloc_contiguous.
        RingMemory::new(rx_ring_virt.as_u64() as *mut u8, rx_ring_phys.as_u64(), rx_layout.total_size)
    } {
        Some(m) => m,
        None => {
            kprintln!("[virtio-net] failed to create RX ring memory");
            return;
        }
    };

    let mut rx_queue = match SplitQueue::new(rx_layout, rx_ring_mem) {
        Some(q) => q,
        None => {
            kprintln!("[virtio-net] failed to create RX split queue");
            return;
        }
    };

    if setup_queue(&transport, 0, &rx_queue, 0).is_err() {
        kprintln!("[virtio-net] failed to setup RX queue");
        return;
    }

    // Setup TX queue (queue 1)
    let tx_queue_size = match queue_size_for(&transport, 1, 64) {
        Ok(s) => {
            kprintln!("[virtio-net] TX queue size: {}", s);
            s
        }
        Err(e) => {
            kprintln!("[virtio-net] TX queue size query failed: {:?}", e);
            return;
        }
    };

    let tx_layout = match Layout::for_size(tx_queue_size) {
        Some(l) => l,
        None => {
            kprintln!("[virtio-net] invalid TX queue size");
            return;
        }
    };

    let tx_ring_frames = tx_layout.total_size.div_ceil(FRAME_SIZE);
    let tx_ring_phys = match pmm::alloc_contiguous(tx_ring_frames, 1) {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] failed to allocate TX ring memory");
            return;
        }
    };

    let tx_ring_virt = crate::mm::hhdm::phys_to_virt(tx_ring_phys);
    let tx_ring_mem = match unsafe {
        // SAFETY: ring_virt is a valid HHDM mapping of ring_phys, which is contiguous memory
        // allocated by pmm::alloc_contiguous.
        RingMemory::new(tx_ring_virt.as_u64() as *mut u8, tx_ring_phys.as_u64(), tx_layout.total_size)
    } {
        Some(m) => m,
        None => {
            kprintln!("[virtio-net] failed to create TX ring memory");
            return;
        }
    };

    let tx_queue = match SplitQueue::new(tx_layout, tx_ring_mem) {
        Some(q) => q,
        None => {
            kprintln!("[virtio-net] failed to create TX split queue");
            return;
        }
    };

    if setup_queue(&transport, 1, &tx_queue, 0).is_err() {
        kprintln!("[virtio-net] failed to setup TX queue");
        return;
    }

    finish_init(&transport);
    kprintln!("[virtio-net] finished init");

    // Allocate RX buffers (2 KiB each, 64 total)
    let rx_buf_size = 2048usize;
    let rx_buf_frames = rx_buf_size.div_ceil(FRAME_SIZE);
    let mut rx_buffers: [PhysAddr; 64] = [PhysAddr::new(0); 64];
    let mut rx_virt_buffers: [VirtAddr; 64] = [VirtAddr::new(0); 64];

    for (i, (buf_phys, buf_virt)) in rx_buffers.iter_mut().zip(&mut rx_virt_buffers).enumerate() {
        let frames = match pmm::alloc_contiguous(rx_buf_frames, 1) {
            Some(f) => f,
            None => {
                kprintln!("[virtio-net] failed to allocate RX buffer {}", i);
                return;
            }
        };
        *buf_phys = frames;
        *buf_virt = crate::mm::hhdm::phys_to_virt(frames);
    }

    // Post RX buffers to the device (device writes header + frame)
    for (i, buf_phys) in rx_buffers.iter().enumerate() {
        let buf = Buffer {
            addr: buf_phys.as_u64(),
            len: rx_buf_size as u32,
            device_writes: true,
        };
        if rx_queue.add(&[buf], i as u64).is_err() {
            kprintln!("[virtio-net] failed to post RX buffer {}", i);
            return;
        }
    }

    // Read queue notify offsets (needed later for correct notification addresses).
    // SAFETY: transport.select_queue and queue_notify_off just read/write MMIO registers.
    transport.select_queue(0);
    let rx_queue_notify_off = transport.queue_notify_off();
    transport.select_queue(1);
    let tx_queue_notify_off = transport.queue_notify_off();

    // Notify RX queue (queue 0) that we've posted buffers.
    transport.select_queue(0);
    transport.notify(0, rx_queue_notify_off);
    kprintln!("[virtio-net] posted 64 RX buffers");

    // Allocate TX bounce buffer (2 KiB + header)
    let tx_bounce_size: usize = 2048 + 12;
    let tx_bounce_frames = tx_bounce_size.div_ceil(FRAME_SIZE);
    let tx_bounce_phys = match pmm::alloc_contiguous(tx_bounce_frames, 1) {
        Some(a) => a,
        None => {
            kprintln!("[virtio-net] failed to allocate TX bounce buffer");
            return;
        }
    };

    let tx_bounce_virt = crate::mm::hhdm::phys_to_virt(tx_bounce_phys);

    let dev = VirtioNet {
        transport,
        rx_queue,
        tx_queue,
        mac,
        rx_buffers,
        rx_virt_buffers,
        tx_bounce_buf_phys: tx_bounce_phys,
        tx_bounce_buf_virt: tx_bounce_virt,
        features: negotiated,
        rx_queue_notify_off,
        tx_queue_notify_off,
        rx_count: 0,
        tx_count: 0,
    };

    let mut guard = DEVICE.lock();
    *guard = VirtioNetDevice { device: Some(Box::new(dev)) };
    kprintln!("[virtio-net] device stored in global");
}

/// Get a reference to the virtio-net device.
pub fn device() -> Option<&'static VirtioNet> {
    // SAFETY: The device is initialized once at boot and never moved or freed.
    unsafe {
        let device_ref = &*(&DEVICE as *const IrqMutex<VirtioNetDevice>);
        let guard = device_ref.lock();
        guard.device.as_ref().map(|b| {
            let b_ref = b.as_ref();
            core::mem::transmute::<&VirtioNet, &'static VirtioNet>(b_ref)
        })
    }
}

impl VirtioNet {
    /// Transmit a frame through the TX queue.
    pub fn transmit(&self, frame: &[u8]) -> Result<(), NetError> {
        if frame.len() > 2048 {
            return Err(NetError::InvalidLength);
        }

        let mut guard = DEVICE.lock();
        let dev = guard.device.as_mut().ok_or(NetError::Io(99))?;

        // Copy header and frame to bounce buffer
        let header = VirtioNetHeader::new();
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &header as *const _ as *const u8,
                core::mem::size_of::<VirtioNetHeader>(),
            )
        };

        // SAFETY: tx_bounce_virt is a valid HHDM pointer to the bounce buffer.
        unsafe {
            core::ptr::copy_nonoverlapping(header_bytes.as_ptr(), dev.tx_bounce_buf_virt.as_u64() as *mut u8, 12);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), (dev.tx_bounce_buf_virt.as_u64() + 12) as *mut u8, frame.len());
        }

        let buffers = [
            Buffer { addr: dev.tx_bounce_buf_phys.as_u64(), len: 12, device_writes: false },
            Buffer { addr: dev.tx_bounce_buf_phys.as_u64() + 12, len: frame.len() as u32, device_writes: false },
        ];

        dev.tx_queue.add(&buffers, dev.tx_count).map_err(|_| NetError::Io(99))?;
        if dev.tx_queue.should_notify() {
            // SAFETY: tx_queue_notify_off was read and stored during init. We just use it here
            // for the correct virtio notification address calculation.
            dev.transport.notify(1, dev.tx_queue_notify_off);
        }
        dev.tx_count += 1;

        drop(guard);

        // Poll for completion
        let deadline = time::uptime_ms() + 1000;
        loop {
            let mut guard = DEVICE.lock();
            let dev = guard.device.as_mut().ok_or(NetError::Io(99))?;

            if let Some((_, _)) = dev.tx_queue.pop_used() {
                drop(guard);
                return Ok(());
            }
            drop(guard);

            if time::uptime_ms() >= deadline {
                return Err(NetError::Timeout);
            }
            core::hint::spin_loop();
        }
    }

    /// Receive a frame from the RX queue. Returns (frame_data, frame_size).
    pub fn receive(&self) -> Option<(VirtAddr, usize)> {
        let mut guard = DEVICE.lock();
        let dev = guard.device.as_mut()?;

        if let Some((idx, len)) = dev.rx_queue.pop_used() {
            let frame_addr = dev.rx_virt_buffers[idx as usize];
            let frame_len = len as usize;
            drop(guard);
            return Some((frame_addr, frame_len));
        }
        drop(guard);
        None
    }

    /// Repost an RX buffer to the device.
    pub fn repost_rx_buffer(&self, idx: usize) {
        let mut guard = DEVICE.lock();
        let dev = guard.device.as_mut().unwrap();

        let buf = Buffer {
            addr: dev.rx_buffers[idx].as_u64(),
            len: 2048u32,
            device_writes: true,
        };

        if dev.rx_queue.add(&[buf], idx as u64).is_ok() && dev.rx_queue.should_notify() {
            // SAFETY: rx_queue_notify_off was read and stored during init.
            dev.transport.notify(0, dev.rx_queue_notify_off);
        }
    }

    /// Get MAC address.
    pub fn get_mac(&self) -> [u8; 6] {
        self.mac
    }

    /// Get RX buffer virtual address by index.
    pub fn get_rx_buffer_virt(&self, idx: usize) -> Option<VirtAddr> {
        if idx < 64 {
            Some(self.rx_virt_buffers[idx])
        } else {
            None
        }
    }

    /// Get device statistics.
    pub fn rx_count(&self) -> u64 {
        let guard = DEVICE.lock();
        guard.device.as_ref().map(|b| b.rx_count).unwrap_or(0)
    }

    pub fn tx_count(&self) -> u64 {
        let guard = DEVICE.lock();
        guard.device.as_ref().map(|b| b.tx_count).unwrap_or(0)
    }
}
