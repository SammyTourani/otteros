//! Virtio split virtqueue implementation (brief M3-T1).
//! A virtqueue consists of three parts: a descriptor table, an available ring,
//! and a used ring. The driver uses the descriptor table and available ring
//! to offer work to the device, and the device uses the used ring to report
//! completion.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU16, Ordering};

/// A single descriptor table entry (16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct Descriptor {
    addr: u64,      // Physical address of buffer
    len: u32,       // Buffer length
    flags: u16,     // Descriptor flags
    next: u16,      // Index of next descriptor (if NEXT flag is set)
}

// Descriptor flags.
const DESC_F_NEXT: u16 = 1;     // This descriptor is followed by another.
const DESC_F_WRITE: u16 = 2;    // Descriptor is write-only (device to driver).
#[allow(dead_code)]
const DESC_F_INDIRECT: u16 = 4; // Descriptor table is indirect (not used here).

/// Available ring entry (just a u16 index into the descriptor table).
type AvailEntry = u16;

/// Used ring entry (8 bytes: descriptor index + written length).
#[repr(C)]
#[derive(Clone, Copy)]
struct UsedEntry {
    id: u32,        // Descriptor index
    len: u32,       // Number of bytes written
}

/// A split virtqueue for device I/O.
pub struct VirtQueue {
    size: u16,
    // Descriptor table: size * 16 bytes
    descriptors: NonNull<Descriptor>,
    // Available ring: 6 + 2*size bytes (flags, index, entries[size], used_event)
    available: NonNull<AvailEntry>,
    #[allow(dead_code)]
    avail_flags: *mut u16,
    avail_idx: *mut u16,
    #[allow(dead_code)]
    avail_used_event: *mut u16,
    // Used ring: 6 + 8*size bytes (flags, index, entries[size], avail_event)
    used: NonNull<UsedEntry>,
    #[allow(dead_code)]
    used_flags: *mut u16,
    used_idx: *mut u16,
    #[allow(dead_code)]
    used_avail_event: *mut u16,

    // Our tracking.
    free_list: AtomicU16,          // Next descriptor index to allocate
    used_idx_cached: AtomicU16,    // Last index we've read from the used ring
}

impl VirtQueue {
    /// Allocates a new virtqueue with `size` descriptors.
    /// Returns None if allocation fails or if size is invalid.
    pub fn new(size: u16) -> Option<Box<VirtQueue>> {
        if size == 0 || size > 32768 {
            return None;
        }

        // Allocate descriptor table (16 bytes per descriptor) as a Vec, then leak it.
        let descriptors: Vec<Descriptor> = alloc::vec![Descriptor { addr: 0, len: 0, flags: 0, next: 0 }; size as usize];
        let desc_ptr = NonNull::new(descriptors.leak().as_mut_ptr())?;

        // Allocate available ring (2 bytes header + 2 bytes per descriptor + 2 bytes trailer).
        let available: Vec<u16> = alloc::vec![0; 2 + size as usize + 1];
        let avail_ptr = NonNull::new(available.leak().as_mut_ptr())?;

        // Allocate used ring (2 bytes header + entries + 2 bytes trailer).
        // Used entries are 8 bytes each (id + len).
        let used: Vec<u8> = alloc::vec![0; 4 + (size as usize * 8) + 2];
        let used_ptr = NonNull::new(used.leak().as_mut_ptr() as *mut UsedEntry)?;

        // Set up pointers into the allocated regions.
        let avail_flags = avail_ptr.as_ptr();
        let avail_idx = unsafe { avail_ptr.as_ptr().add(1) };
        let _avail_entries = unsafe { avail_ptr.as_ptr().add(2) };
        let avail_used_event = unsafe { avail_ptr.as_ptr().add(2 + size as usize) };

        let used_flags = used_ptr.as_ptr() as *mut u16;
        let used_idx = unsafe { (used_ptr.as_ptr() as *mut u16).add(1) };
        let _used_entries = unsafe { used_ptr.as_ptr().add(1) };
        let used_avail_event = unsafe { (used_ptr.as_ptr() as *mut u16).add(2 + (size as usize * 4)) };

        Some(Box::new(VirtQueue {
            size,
            descriptors: desc_ptr,
            available: avail_ptr,
            avail_flags,
            avail_idx,
            avail_used_event,
            used: used_ptr,
            used_flags,
            used_idx,
            used_avail_event,
            free_list: AtomicU16::new(0),
            used_idx_cached: AtomicU16::new(0),
        }))
    }

    /// Returns the physical address of the descriptor table.
    pub fn desc_phys_addr(&self) -> u64 {
        self.descriptors.as_ptr() as u64 // Assumes kernel memory is identity-mapped
    }

    /// Returns the physical address of the available ring.
    pub fn avail_phys_addr(&self) -> u64 {
        self.available.as_ptr() as u64
    }

    /// Returns the physical address of the used ring.
    pub fn used_phys_addr(&self) -> u64 {
        self.used.as_ptr() as u64
    }

    /// Returns the size of the virtqueue.
    pub fn size(&self) -> u16 {
        self.size
    }

    /// Allocates a descriptor and returns its index, or None if the queue is full.
    pub fn alloc_descriptor(&self) -> Option<u16> {
        let idx = self.free_list.load(Ordering::Acquire);
        if idx >= self.size {
            return None;
        }
        let next = (idx + 1) % self.size;
        self.free_list.store(next, Ordering::Release);
        Some(idx)
    }

    /// Sets up a descriptor chain. `descriptors` is a list of (address, len, write).
    /// Returns the index of the first descriptor, or None if not enough descriptors.
    pub fn chain_descriptors(&self, descriptors: &[(u64, u32, bool)]) -> Option<u16> {
        if descriptors.is_empty() {
            return None;
        }

        let first_idx = self.alloc_descriptor()?;

        for (i, &(addr, len, write)) in descriptors.iter().enumerate() {
            let idx = if i == 0 { first_idx } else { self.alloc_descriptor()? };

            let mut flags = 0;
            if write {
                flags |= DESC_F_WRITE;
            }
            if i < descriptors.len() - 1 {
                flags |= DESC_F_NEXT;
            }

            unsafe {
                let desc = self.descriptors.as_ptr().add(idx as usize);
                (*desc).addr = addr;
                (*desc).len = len;
                (*desc).flags = flags;
                (*desc).next = if i < descriptors.len() - 1 { idx + 1 } else { 0 };
            }
        }

        Some(first_idx)
    }

    /// Submits a descriptor chain by adding it to the available ring.
    pub fn submit(&self, desc_idx: u16) {
        let avail_idx = unsafe { *self.avail_idx };
        let idx_in_ring = (avail_idx % self.size) as usize;
        unsafe {
            let entry = self.available.as_ptr().add(2 + idx_in_ring);
            *entry = desc_idx;
        }
        // Increment the available index.
        unsafe {
            *self.avail_idx = avail_idx.wrapping_add(1);
        }
    }

    /// Checks if there's a pending completion and returns (descriptor_id, written_bytes)
    /// or None if no completion is available.
    pub fn get_completion(&self) -> Option<(u32, u32)> {
        let used_idx = unsafe { *self.used_idx };
        let cached_idx = self.used_idx_cached.load(Ordering::Acquire);

        if used_idx == cached_idx {
            return None;
        }

        let idx_in_ring = (cached_idx % self.size) as usize;
        let entry = unsafe { self.used.as_ptr().add(idx_in_ring).read() };
        self.used_idx_cached.store(cached_idx.wrapping_add(1), Ordering::Release);

        Some((entry.id, entry.len))
    }

    /// Resets the queue for reuse (clears all descriptors and rings).
    pub fn reset(&self) {
        self.free_list.store(0, Ordering::Release);
        self.used_idx_cached.store(0, Ordering::Release);

        unsafe {
            *self.avail_idx = 0;
            *self.used_idx = 0;
        }
    }
}

// NOTE: VirtQueue memory is leaked on drop (acceptable for now since virtqueues
// are typically long-lived and there are few of them per device).
