#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

// ============================================================================
// Feature and Status Constants (§2.1, §6)
// ============================================================================

pub const VIRTIO_F_INDIRECT_DESC: u64 = 1u64 << 28;
pub const VIRTIO_F_RING_EVENT_IDX: u64 = 1u64 << 29;
pub const VIRTIO_F_VERSION_1: u64 = 1u64 << 32;

pub const STATUS_ACKNOWLEDGE: u8 = 1;
pub const STATUS_DRIVER: u8 = 2;
pub const STATUS_FEATURES_OK: u8 = 8;
pub const STATUS_DRIVER_OK: u8 = 4;
pub const STATUS_FAILED: u8 = 128;

// ============================================================================
// Layout (§2.7)
// ============================================================================

/// Memory layout for a split virtqueue.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub desc_offset: usize,
    pub avail_offset: usize,
    pub used_offset: usize,
    pub total_size: usize,
    pub queue_size: u16,
}

impl Layout {
    /// Calculate layout for a given queue size (power of two, 1..=32768).
    pub fn for_size(n: u16) -> Option<Self> {
        if n == 0 || n > 32768 || (n & (n - 1)) != 0 {
            return None;
        }
        let n = n as usize;
        let desc_offset = 0;
        let avail_offset = desc_offset + 16 * n;
        let used_offset = (avail_offset + 6 + 2 * n + 3) & !3;
        let total_size = used_offset + 6 + 8 * n;
        Some(Layout {
            desc_offset,
            avail_offset,
            used_offset,
            total_size,
            queue_size: n as u16,
        })
    }
}

// ============================================================================
// Ring Memory
// ============================================================================

/// Ring memory: virtual and physical addresses.
pub struct RingMemory {
    virt: NonNull<u8>,
    phys: u64,
    len: usize,
}

impl RingMemory {
    /// Create from virtual and physical addresses.
    /// # Safety
    /// Caller must ensure virt points to valid, aligned, exclusive memory.
    pub unsafe fn new(virt: *mut u8, phys: u64, len: usize) -> Option<Self> {
        Some(RingMemory {
            virt: NonNull::new(virt)?,
            phys,
            len,
        })
    }

    fn virt(&self) -> *mut u8 {
        self.virt.as_ptr()
    }

    fn phys(&self) -> u64 {
        self.phys
    }
}

// ============================================================================
// Descriptor Layout (§2.7.5)
// ============================================================================

#[repr(C)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

#[repr(C)]
struct UsedEntry {
    id: u32,
    len: u32,
}

// ============================================================================
// Buffer Descriptor
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct Buffer {
    pub addr: u64,
    pub len: u32,
    pub device_writes: bool,
}

// ============================================================================
// Split Queue (§2.7)
// ============================================================================

pub struct SplitQueue {
    layout: Layout,
    ring_mem: RingMemory,
    free_descs: Vec<u16>,
    num_free: usize,
    next_avail: u16,
    next_used: u16,
    last_notify_idx: u16,
    event_idx_enabled: bool,
    token_map: Vec<u64>,
}

impl SplitQueue {
    /// Create a new split queue.
    pub fn new(layout: Layout, ring_mem: RingMemory) -> Option<Self> {
        if ring_mem.len < layout.total_size {
            return None;
        }
        let queue_size = layout.queue_size as usize;
        let free_descs: Vec<u16> = (0..queue_size).map(|i| i as u16).collect();
        let token_map: Vec<u64> = alloc::vec![0; queue_size];

        // SAFETY: ring_mem is valid and we verified its size
        unsafe {
            core::ptr::write_bytes(ring_mem.virt(), 0, ring_mem.len);
        }

        Some(SplitQueue {
            layout,
            ring_mem,
            free_descs,
            num_free: queue_size,
            next_avail: 0,
            next_used: 0,
            last_notify_idx: 0,
            event_idx_enabled: false,
            token_map,
        })
    }

    pub fn desc_phys_addr(&self) -> u64 {
        self.ring_mem.phys() + self.layout.desc_offset as u64
    }

    pub fn avail_phys_addr(&self) -> u64 {
        self.ring_mem.phys() + self.layout.avail_offset as u64
    }

    pub fn used_phys_addr(&self) -> u64 {
        self.ring_mem.phys() + self.layout.used_offset as u64
    }

    pub fn free_descriptors(&self) -> usize {
        self.num_free
    }

    pub fn add(&mut self, buffers: &[Buffer], token: u64) -> Result<(), AddError> {
        if buffers.is_empty() {
            return Err(AddError::EmptyBuffers);
        }
        let needed = buffers.len();
        if self.num_free < needed {
            return Err(AddError::QueueFull);
        }

        // Allocate descriptors from the free pool
        let allocated: Vec<u16> = (0..needed).map(|i| self.free_descs[self.num_free - 1 - i]).collect();
        self.num_free -= needed;

        let base = self.ring_mem.virt() as usize;

        // Build descriptor chain
        for (i, &desc_idx) in allocated.iter().enumerate() {
            let buffer = &buffers[i];
            let mut flags = 0u16;
            if buffer.device_writes {
                flags |= DESC_F_WRITE;
            }
            if i < allocated.len() - 1 {
                flags |= DESC_F_NEXT;
            }
            let next = if i < allocated.len() - 1 {
                allocated[i + 1]
            } else {
                0
            };

            let desc_off = self.layout.desc_offset + (desc_idx as usize) * 16;
            let desc_ptr = (base + desc_off) as *mut Descriptor;

            // SAFETY: desc_idx is in bounds and memory is owned
            unsafe {
                (*desc_ptr).addr = buffer.addr;
                (*desc_ptr).len = buffer.len;
                (*desc_ptr).flags = flags;
                (*desc_ptr).next = next;
            }
        }

        let head = allocated[0];
        self.token_map[head as usize] = token;

        // Publish to available ring with Release ordering
        let ring_idx = self.next_avail;
        let entry_off = self.layout.avail_offset + 4 + ((ring_idx as usize % self.layout.queue_size as usize) * 2);
        let entry_ptr = (base + entry_off) as *mut u16;

        // SAFETY: entry_off is in bounds
        unsafe {
            core::ptr::write_volatile(entry_ptr, head);
            core::sync::atomic::compiler_fence(Ordering::Release);
        }

        self.next_avail = self.next_avail.wrapping_add(1);

        let idx_off = self.layout.avail_offset + 2;
        let idx_ptr = (base + idx_off) as *mut core::sync::atomic::AtomicU16;

        // SAFETY: idx_off is in bounds and properly aligned for u16
        unsafe {
            (*idx_ptr).store(self.next_avail, Ordering::Release);
        }

        Ok(())
    }

    pub fn pop_used(&mut self) -> Option<(u64, u32)> {
        let base = self.ring_mem.virt() as usize;

        let idx_off = self.layout.used_offset + 2;
        let idx_ptr = (base + idx_off) as *mut core::sync::atomic::AtomicU16;

        // SAFETY: idx_off is in bounds and properly aligned for u16
        let device_used_idx = unsafe { (*idx_ptr).load(Ordering::Acquire) };

        if device_used_idx == self.next_used {
            return None;
        }

        let entry_idx = self.next_used as usize % self.layout.queue_size as usize;
        let entry_off = self.layout.used_offset + 4 + entry_idx * 8;
        let entry_ptr = (base + entry_off) as *const UsedEntry;

        // SAFETY: entry_off is in bounds
        let entry = unsafe { core::ptr::read_volatile(entry_ptr) };

        self.next_used = self.next_used.wrapping_add(1);

        let desc_idx = entry.id as u16;
        let token = self.token_map[desc_idx as usize];
        self.return_descriptor_chain(desc_idx);

        Some((token, entry.len))
    }

    fn return_descriptor_chain(&mut self, mut desc_idx: u16) {
        let base = self.ring_mem.virt() as usize;
        loop {
            let desc_off = self.layout.desc_offset + (desc_idx as usize) * 16;
            let desc_ptr = (base + desc_off) as *const Descriptor;

            // SAFETY: desc_off is in bounds
            let descriptor = unsafe { core::ptr::read_volatile(desc_ptr) };

            // Return descriptor to the free pool
            self.free_descs[self.num_free] = desc_idx;
            self.num_free += 1;

            if (descriptor.flags & DESC_F_NEXT) == 0 {
                break;
            }
            desc_idx = descriptor.next;
        }
    }

    pub fn should_notify(&mut self) -> bool {
        // The avail.idx store must be visible to the device before we read its suppression state
        // (used.flags or avail_event): a store followed by a load from another address may be
        // reordered by the CPU (x86 store buffer), so this needs a hardware barrier, not only a
        // compiler fence (virtio 1.2 §2.7.13.4 and §2.7.10; Linux virtio_mb in kick_prepare).
        core::sync::atomic::fence(Ordering::SeqCst);
        if !self.event_idx_enabled {
            // Without EVENT_IDX: check VIRTQ_USED_F_NO_NOTIFY in used ring flags
            let base = self.ring_mem.virt() as usize;
            let flags_off = self.layout.used_offset;
            let flags_ptr = (base + flags_off) as *const u16;

            // SAFETY: flags_off is in bounds
            let flags = unsafe { core::ptr::read_volatile(flags_ptr) };
            (flags & 1) == 0
        } else {
            // With EVENT_IDX: vring_need_event, read avail_event from used ring
            let base = self.ring_mem.virt() as usize;
            let event_off = self.layout.used_offset + 4 + (self.layout.queue_size as usize * 8);
            let event_ptr = (base + event_off) as *const core::sync::atomic::AtomicU16;

            // SAFETY: event_off is in bounds and 2-aligned (spec layout); the full fence at the top of this function orders the load.
            let avail_event = unsafe { (*event_ptr).load(Ordering::Relaxed) };

            let new = self.next_avail;
            let old = self.last_notify_idx;
            self.last_notify_idx = new;

            new.wrapping_sub(avail_event).wrapping_sub(1) < new.wrapping_sub(old)
        }
    }

    pub fn set_used_event(&mut self, v: u16) {
        let base = self.ring_mem.virt() as usize;
        let off = self.layout.avail_offset + 4 + (self.layout.queue_size as usize * 2);
        let ptr = (base + off) as *mut u16;

        // SAFETY: off is in bounds
        unsafe {
            core::ptr::write_volatile(ptr, v);
        }
    }

    pub fn set_event_idx_enabled(&mut self, enabled: bool) {
        self.event_idx_enabled = enabled;
    }
}

// ============================================================================
// Error Types
// ============================================================================

#[derive(Debug)]
pub enum AddError {
    EmptyBuffers,
    QueueFull,
}

#[derive(Debug, Clone, Copy)]
pub enum InitError {
    NoVersion1,
    FeaturesRejected,
    NoSuchQueue,
    ZeroMaxQueueSize,
    MsixNotAccepted,
}

// ============================================================================
// Transport Trait (§3.1, §4.1.4.3)
// ============================================================================

pub trait Transport {
    fn device_feature(&self, select: u32) -> u32;
    fn set_driver_feature(&self, select: u32, value: u32);
    fn status(&self) -> u8;
    fn set_status(&self, s: u8);
    fn num_queues(&self) -> u16;
    fn select_queue(&self, index: u16);
    fn queue_size(&self) -> u16;
    fn set_queue_size(&self, size: u16);
    fn set_queue_msix_vector(&self, v: u16);
    fn queue_msix_vector(&self) -> u16;
    fn set_queue_addresses(&self, desc: u64, driver: u64, device: u64);
    fn set_queue_enable(&self, enable: bool);
    fn queue_notify_off(&self) -> u16;
    fn config_generation(&self) -> u32;
    fn read_config(&self, offset: usize, buf: &mut [u8]);
    fn notify(&self, queue: u16, notify_off: u16);
}

// ============================================================================
// Initialization (§3.1.1)
// ============================================================================

pub fn negotiate(t: &dyn Transport, wanted: u64) -> Result<u64, InitError> {
    // Reset
    t.set_status(0);

    // ACKNOWLEDGE
    t.set_status(STATUS_ACKNOWLEDGE);

    // DRIVER
    t.set_status(STATUS_ACKNOWLEDGE | STATUS_DRIVER);

    // Read features (both 32-bit words)
    let low = t.device_feature(0) as u64;
    let high = (t.device_feature(1) as u64) << 32;
    let offered = low | high;

    // Require VERSION_1
    if (offered & VIRTIO_F_VERSION_1) == 0 {
        t.set_status(STATUS_FAILED);
        return Err(InitError::NoVersion1);
    }

    // Negotiate: offered & (wanted | VERSION_1)
    let negotiated = offered & (wanted | VIRTIO_F_VERSION_1);

    // Write features (both 32-bit words)
    t.set_driver_feature(0, (negotiated & 0xffff_ffff) as u32);
    t.set_driver_feature(1, (negotiated >> 32) as u32);

    // Set FEATURES_OK
    t.set_status(STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);

    // Re-read FEATURES_OK
    if (t.status() & STATUS_FEATURES_OK) == 0 {
        t.set_status(STATUS_FAILED);
        return Err(InitError::FeaturesRejected);
    }

    Ok(negotiated)
}

pub fn queue_size_for(t: &dyn Transport, index: u16, desired: u16) -> Result<u16, InitError> {
    if index >= t.num_queues() {
        return Err(InitError::NoSuchQueue);
    }

    t.select_queue(index);
    let max = t.queue_size();

    if max == 0 {
        return Err(InitError::ZeroMaxQueueSize);
    }

    // Return min(desired, max), as a power of two
    let size = desired.min(max);
    let mut result = 1u16;
    while result < size {
        result <<= 1;
    }
    Ok(result.min(max))
}

pub fn setup_queue(t: &dyn Transport, index: u16, q: &SplitQueue, msix_vector: u16) -> Result<u16, InitError> {
    t.select_queue(index);

    // Set queue size
    t.set_queue_size(q.layout.queue_size);

    // Set addresses
    t.set_queue_addresses(q.desc_phys_addr(), q.avail_phys_addr(), q.used_phys_addr());

    // Set MSI-X vector and read back
    t.set_queue_msix_vector(msix_vector);
    if t.queue_msix_vector() == 0xffff {
        return Err(InitError::MsixNotAccepted);
    }

    // Read queue_notify_off
    let notify_off = t.queue_notify_off();

    // Enable queue (last)
    t.set_queue_enable(true);

    Ok(notify_off)
}

pub fn finish_init(t: &dyn Transport) {
    let s = t.status();
    t.set_status(s | STATUS_DRIVER_OK);
}

pub fn read_config_consistent(t: &dyn Transport, offset: usize, buf: &mut [u8]) {
    loop {
        let gen1 = t.config_generation();
        t.read_config(offset, buf);
        let gen2 = t.config_generation();
        if gen1 == gen2 {
            break;
        }
    }
}

// ============================================================================
// Block Device Helpers (§5.2.6)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRequest {
    pub req_type: u32,
    pub reserved: u32,
    pub sector: u64,
}

impl BlockRequest {
    pub fn new(req_type: u32, sector: u64) -> Self {
        BlockRequest {
            req_type,
            reserved: 0,
            sector,
        }
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&self.req_type.to_le_bytes());
        b[4..8].copy_from_slice(&self.reserved.to_le_bytes());
        b[8..16].copy_from_slice(&self.sector.to_le_bytes());
        b
    }
}

pub const BLOCK_T_IN: u32 = 0;
pub const BLOCK_T_OUT: u32 = 1;
pub const BLOCK_T_FLUSH: u32 = 4;
pub const BLOCK_T_GET_ID: u32 = 8;

pub const BLOCK_S_OK: u32 = 0;
pub const BLOCK_S_IOERR: u32 = 1;
pub const BLOCK_S_UNSUPP: u32 = 2;

// ============================================================================
// Net Device Helpers (§5.1.6)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetHeader {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

impl NetHeader {
    pub fn new() -> Self {
        NetHeader {
            flags: 0,
            gso_type: 0,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
            num_buffers: 0,
        }
    }

    pub fn to_bytes(&self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[0] = self.flags;
        b[1] = self.gso_type;
        b[2..4].copy_from_slice(&self.hdr_len.to_le_bytes());
        b[4..6].copy_from_slice(&self.gso_size.to_le_bytes());
        b[6..8].copy_from_slice(&self.csum_start.to_le_bytes());
        b[8..10].copy_from_slice(&self.csum_offset.to_le_bytes());
        b[10..12].copy_from_slice(&self.num_buffers.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; 12]) -> Self {
        NetHeader {
            flags: b[0],
            gso_type: b[1],
            hdr_len: u16::from_le_bytes([b[2], b[3]]),
            gso_size: u16::from_le_bytes([b[4], b[5]]),
            csum_start: u16::from_le_bytes([b[6], b[7]]),
            csum_offset: u16::from_le_bytes([b[8], b[9]]),
            num_buffers: u16::from_le_bytes([b[10], b[11]]),
        }
    }
}

impl Default for NetHeader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {}
