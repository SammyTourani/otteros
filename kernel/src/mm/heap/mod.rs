//! The kernel heap (brief M1-T3): `#[global_allocator]` for
//! `alloc::{vec, string, boxed, collections}`, backed entirely by frames
//! from the PMM (DECISIONS.md D2 -- written here, not by
//! `linked_list_allocator`/`buddy_system_allocator`, both forbidden).
//!
//! Requests are split into two paths:
//! - **Small** (`size` and `align` both `<= 2048`): served by `slab`, a
//!   size-class allocator over 4 KiB frames.
//! - **Large** (`size > 2048` or `align > 2048`, up to `align == 4096`):
//!   served by `large`, straight from `pmm::alloc_contiguous`. Alignment
//!   above 4096 is unsupported and `alloc` returns null for it, same as
//!   any other allocation failure -- never a panic inside `alloc` itself.
//!
//! Both paths, and the running stats, are protected by one lock
//! (`HEAP`), taken with interrupts disabled (`without_interrupts`) for the
//! same reason `mm::pmm`'s lock is: `spin::Mutex` isn't reentrant, so this
//! is already safe to call from IRQ context once a later task enables
//! interrupts, at no cost today (M1 never does).

mod large;
mod slab;

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use spin::Mutex;

use super::addr::FRAME_SIZE;
use crate::arch::x86_64::interrupts::without_interrupts;
use crate::kprintln;

/// Point-in-time heap counters, read via `heap::stats()`.
pub struct HeapStats {
    /// Total bytes currently backing live allocations: for small objects,
    /// the sum of their class sizes (not the smaller requested sizes); for
    /// large objects, `large_frames * FRAME_SIZE`.
    pub bytes_in_use: usize,
    /// Live objects served by the slab path.
    pub small_objects_in_use: usize,
    /// Slab frames currently owned by the slab allocator (across every
    /// class), each returned to the PMM the moment it has no live objects.
    pub slabs: usize,
    /// Live objects served by the large-object path.
    pub large_objects: usize,
    /// Frames currently owned by live large objects.
    pub large_frames: usize,
}

/// How `classify` routes one `Layout`.
enum AllocKind {
    /// Index into `slab::CLASS_SIZES`.
    Small(usize),
    /// Frame count for `pmm::alloc_contiguous`.
    Large { n_frames: usize },
    /// `align` exceeds what this heap ever supports (4096 bytes); `alloc`
    /// must return null rather than attempt this.
    Rejected,
}

/// Decides which path a request takes. `size` is clamped to at least 1
/// (Rust's own allocator entry points never call `GlobalAlloc` for a
/// genuine zero-sized type, but nothing here should misbehave if one ever
/// did); `align` is a power of two by `Layout`'s own invariant.
fn classify(layout: Layout) -> AllocKind {
    let size = layout.size().max(1);
    let align = layout.align();

    if align > FRAME_SIZE {
        return AllocKind::Rejected;
    }

    let required = size.max(align);
    if required > slab::MAX_SLAB_SIZE {
        AllocKind::Large { n_frames: size.div_ceil(FRAME_SIZE).max(1) }
    } else {
        AllocKind::Small(slab::class_index_for(required))
    }
}

/// The heap's mutable state: per-class slab lists, the large-object table,
/// and the running stats they imply. Every method here is safe Rust that
/// assumes exclusive access (i.e. the caller already holds `HEAP`'s lock);
/// the `unsafe` pointer work lives in `slab`/`large`.
struct HeapState {
    classes: [slab::ClassState; slab::NUM_CLASSES],
    large: large::LargeTable,
    bytes_in_use: usize,
    small_objects_in_use: usize,
    slabs: usize,
}

impl HeapState {
    const fn new() -> Self {
        Self {
            classes: [slab::ClassState::new(); slab::NUM_CLASSES],
            large: large::LargeTable::new(),
            bytes_in_use: 0,
            small_objects_in_use: 0,
            slabs: 0,
        }
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        match classify(layout) {
            AllocKind::Small(class_idx) => match slab::alloc_object(&mut self.classes[class_idx], class_idx) {
                Some(outcome) => {
                    if outcome.new_slab {
                        self.slabs += 1;
                    }
                    self.small_objects_in_use += 1;
                    self.bytes_in_use += slab::CLASS_SIZES[class_idx];
                    outcome.ptr
                }
                None => ptr::null_mut(),
            },
            AllocKind::Large { n_frames } => match self.large.alloc(n_frames) {
                Some(ptr) => {
                    self.bytes_in_use += n_frames * FRAME_SIZE;
                    ptr
                }
                None => ptr::null_mut(),
            },
            AllocKind::Rejected => ptr::null_mut(),
        }
    }

    fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        match classify(layout) {
            AllocKind::Small(class_idx) => {
                if slab::free_object(&mut self.classes[class_idx], ptr, class_idx) {
                    self.slabs -= 1;
                }
                self.small_objects_in_use -= 1;
                self.bytes_in_use -= slab::CLASS_SIZES[class_idx];
            }
            AllocKind::Large { .. } => {
                let frames = self.large.free(ptr);
                self.bytes_in_use -= frames * FRAME_SIZE;
            }
            AllocKind::Rejected => {
                // `alloc` never returns a non-null pointer for a `Rejected`
                // layout, and `GlobalAlloc::dealloc`'s contract is that
                // `layout` is exactly what a prior successful `alloc` used
                // -- so a real caller can never reach this arm.
                debug_assert!(false, "heap::dealloc: rejected layout was never allocated");
            }
        }
    }

    fn alloc_zeroed(&mut self, layout: Layout) -> *mut u8 {
        let ptr = self.alloc(layout);
        if !ptr.is_null() {
            // SAFETY: `alloc` just returned `ptr` as valid for exactly
            // `layout.size()` bytes, exclusively owned by the caller from
            // this point on -- zeroing only that much (not the possibly
            // larger backing class/frame size) is exactly what
            // `alloc_zeroed`'s contract requires.
            unsafe { ptr::write_bytes(ptr, 0, layout.size()) };
        }
        ptr
    }

    fn realloc(&mut self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return ptr::null_mut();
        };

        // Fast path: the new size still fits in the same backing block, so
        // the data is already exactly where it needs to be.
        match (classify(layout), classify(new_layout)) {
            (AllocKind::Small(old), AllocKind::Small(new)) if old == new => return ptr,
            (AllocKind::Large { n_frames: old }, AllocKind::Large { n_frames: new }) if old == new => {
                return ptr;
            }
            _ => {}
        }

        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            let copy_len = layout.size().min(new_size);
            // SAFETY: `ptr` is valid for `layout.size()` bytes (the
            // caller's existing, live allocation), `new_ptr` is valid for
            // `new_size` bytes (just allocated above, exclusively ours),
            // the two ranges can't overlap (independent allocations), and
            // `copy_len` doesn't exceed either buffer.
            unsafe { ptr::copy_nonoverlapping(ptr, new_ptr, copy_len) };
            self.dealloc(ptr, layout);
        }
        new_ptr
    }

    fn stats(&self) -> HeapStats {
        HeapStats {
            bytes_in_use: self.bytes_in_use,
            small_objects_in_use: self.small_objects_in_use,
            slabs: self.slabs,
            large_objects: self.large.count,
            large_frames: self.large.total_frames,
        }
    }
}

// SAFETY: `HeapState` contains raw pointers (the slab lists' intrusive
// links), which Rust doesn't infer `Send` for automatically. Every field
// is only ever touched from inside `HEAP`'s lock (see `with_heap`), so
// sharing it across contexts is sound despite that -- the same reasoning
// `mm::pmm::Pmm` relies on implicitly by living behind its own `Mutex`.
unsafe impl Send for HeapState {}

static HEAP: Mutex<HeapState> = Mutex::new(HeapState::new());

/// Locks `HEAP` with interrupts disabled -- see the module docs for why.
fn with_heap<R>(f: impl FnOnce(&mut HeapState) -> R) -> R {
    without_interrupts(|| f(&mut HEAP.lock()))
}

/// Current heap counters.
pub fn stats() -> HeapStats {
    with_heap(|h| h.stats())
}

/// Logs that the heap is ready. The allocator itself needs no runtime
/// setup (every class starts empty, the large-object table starts clear,
/// all of it built by `const fn`s), so this only announces it.
pub fn init() {
    kprintln!("[heap] ready: {} size classes, large objects via PMM", slab::NUM_CLASSES);
}

/// The kernel's `#[global_allocator]`: zero-sized, delegating every
/// operation to `HEAP` (see `with_heap`). `extern crate alloc;` in lib.rs
/// is what routes `alloc::{vec, string, boxed, collections}` through this.
pub struct KernelHeap;

#[global_allocator]
static ALLOCATOR: KernelHeap = KernelHeap;

// SAFETY (trait-level): `alloc`/`alloc_zeroed` never return a dangling or
// unaligned pointer -- `slab::alloc_object` hands out object slots that
// are, by construction (frame-aligned slab base plus a multiple of
// `class_size`), aligned to at least `class_size >= align`, and
// `large::LargeTable::alloc` hands out frame-aligned (>= 4096-byte)
// addresses, both backed by frames the PMM has exclusively marked used
// until the matching `dealloc`/`free_contiguous`. `dealloc`/`realloc`
// forward exactly the `ptr`/`layout` the caller is contractually required
// to have gotten from a matching prior `alloc` call on this same
// allocator, which is what every `SAFETY` comment inside `HeapState`,
// `slab` and `large` above builds on.
unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        with_heap(|h| h.alloc(layout))
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        with_heap(|h| h.dealloc(ptr, layout));
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        with_heap(|h| h.alloc_zeroed(layout))
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        with_heap(|h| h.realloc(ptr, layout, new_size))
    }
}
