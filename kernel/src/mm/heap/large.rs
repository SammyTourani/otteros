//! Bookkeeping for allocations too big for any slab class (brief M1-T3
//! step 2): backed directly by contiguous PMM frames via
//! `pmm::alloc_contiguous`/`free_contiguous`, addressed through the HHDM.
//! A small fixed-capacity table records how many frames each live large
//! allocation owns, so `free` knows how much to give back.
//
// TODO(M1-T4): move large objects to a virtual heap region once the VMM
// exists, instead of this fixed-capacity table over HHDM addresses.

use super::super::addr::VirtAddr;
use super::super::{hhdm, pmm};

/// How many concurrently live large (> 2048-byte, or > 2048-byte-aligned)
/// allocations this table can track at once. Generous for a kernel with no
/// large long-running workload yet; `alloc` fails closed (frees the frames
/// straight back and returns `None`) rather than losing track of one if
/// this is ever full.
const MAX_LARGE_OBJECTS: usize = 512;

/// One live large allocation: its HHDM address and frame count. `virt == 0`
/// marks an empty slot (never a valid HHDM address: frame 0 is never
/// allocated, see `pmm::free_frame`'s docs).
#[derive(Clone, Copy)]
struct Entry {
    virt: u64,
    frames: usize,
}

const EMPTY_ENTRY: Entry = Entry { virt: 0, frames: 0 };

pub struct LargeTable {
    entries: [Entry; MAX_LARGE_OBJECTS],
    pub count: usize,
    pub total_frames: usize,
}

impl LargeTable {
    pub const fn new() -> Self {
        Self { entries: [EMPTY_ENTRY; MAX_LARGE_OBJECTS], count: 0, total_frames: 0 }
    }

    /// Allocates `n_frames` contiguous, frame-aligned frames and records
    /// the mapping so `free` can find it again. `None` if the PMM has no
    /// such run, or this table is already full (`MAX_LARGE_OBJECTS`
    /// concurrent large allocations) -- either way, never a panic.
    pub fn alloc(&mut self, n_frames: usize) -> Option<*mut u8> {
        let slot = self.entries.iter().position(|e| e.virt == 0)?;
        let phys = pmm::alloc_contiguous(n_frames, 1)?;
        let virt = hhdm::phys_to_virt(phys).as_u64();
        self.entries[slot] = Entry { virt, frames: n_frames };
        self.count += 1;
        self.total_frames += n_frames;
        Some(virt as *mut u8)
    }

    /// Frees the large allocation at `ptr` (previously returned by
    /// `alloc`), returning how many frames it held.
    ///
    /// # Panics
    /// If `ptr` isn't a live entry (kernel-review, M1-T3 fix #2) --
    /// `HeapState::dealloc` only ever calls this with a pointer this table
    /// itself returned from `alloc`, so reaching here means the caller
    /// passed a pointer/layout this allocator never produced.
    pub fn free(&mut self, ptr: *mut u8) -> usize {
        let virt = ptr as u64;
        let slot = self
            .entries
            .iter()
            .position(|e| e.virt == virt)
            .unwrap_or_else(|| panic!("heap: free of unknown large object at 0x{virt:x}"));
        let frames = self.entries[slot].frames;
        self.entries[slot] = EMPTY_ENTRY;
        self.count -= 1;
        self.total_frames -= frames;

        let phys = hhdm::virt_to_phys_hhdm(VirtAddr::new(virt));
        pmm::free_contiguous(phys, frames);
        frames
    }
}
