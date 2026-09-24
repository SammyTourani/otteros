//! The global allocator (brief M2-T3 step 5): size-class free lists for
//! small objects, direct `map_anon`/`unmap` for large ones -- both backed
//! by the kernel's anonymous-memory syscalls, never a bump allocator over
//! a fixed static buffer.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr;

use crate::mem;
use crate::sync::SpinLock;

/// Every class is a power of two, and (since `map_anon`/a slab's own base
/// are always page-aligned, and every class size divides 4096 evenly) a
/// block's address is always a multiple of its own class size -- so
/// picking the smallest class `>= max(size, align)` satisfies both the
/// size *and* the alignment `Layout` asks for, with no separate alignment
/// bookkeeping needed.
const CLASS_SIZES: [usize; 12] = [16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768];

/// Refill granularity for every size class (brief step 5: "size-class free
/// lists for small objects"). Anything needing more than the largest class
/// goes through `alloc_large`/`dealloc_large` instead (brief: "direct
/// `map_anon` for >= 64 KiB" -- the largest class tops out at 32 KiB, so
/// the two paths meet with no gap).
const SLAB_SIZE: usize = 64 * 1024;

struct Classes {
    /// One intrusive free-list head per class (`CLASS_SIZES[i]`), `null`
    /// when empty. A free block's own first 8 bytes hold the next pointer
    /// (or null) -- valid because every class is at least 16 bytes.
    heads: [*mut u8; CLASS_SIZES.len()],
}

// SAFETY: every pointer `Classes` stores is only ever read or written
// while holding the `SpinLock` that guards it (see `sync::SpinLock`'s own
// `unsafe impl Sync` docs for the matching half of this reasoning) --
// `Send` is all a `SpinLock<T>` needs from its payload, and nothing here
// is thread-local or otherwise unsafe to move between threads.
unsafe impl Send for Classes {}

static CLASSES: SpinLock<Classes> = SpinLock::new(Classes { heads: [ptr::null_mut(); CLASS_SIZES.len()] });

pub struct OtterAlloc;

#[global_allocator]
static ALLOCATOR: OtterAlloc = OtterAlloc;

fn page_round(len: usize) -> usize {
    len.next_multiple_of(4096)
}

/// The size class that satisfies both `layout.size()` and `layout.align()`
/// at once, or `None` if `layout` needs more than the largest class (the
/// caller should use the direct `map_anon` path instead).
fn class_for(layout: Layout) -> Option<usize> {
    let need = layout.size().max(layout.align());
    CLASS_SIZES.iter().position(|&class_size| class_size >= need)
}

/// Maps a fresh `SLAB_SIZE` slab and carves it into `CLASS_SIZES[class]`-
/// sized blocks, linking every one onto `classes.heads[class]`. A
/// deliberate, documented simplification (the same shape as this
/// project's other already-accepted ones, e.g. `mm::paging`'s unfreed
/// intermediate page tables): a slab that becomes entirely free again is
/// never `unmap`'d back -- its blocks simply stay on the free list for the
/// next allocation of the same class, rather than this allocator tracking
/// per-slab occupancy to reclaim it.
fn refill(classes: &mut Classes, class: usize) -> bool {
    let Ok(base) = mem::map_anon(SLAB_SIZE) else { return false };
    let block_size = CLASS_SIZES[class];
    let count = SLAB_SIZE / block_size;
    for i in (0..count).rev() {
        // SAFETY: `base` is a fresh, exclusively-owned `SLAB_SIZE`-byte
        // mapping this call alone just obtained from `map_anon`; `i *
        // block_size` never reaches `SLAB_SIZE` (`i < count == SLAB_SIZE /
        // block_size`), and every block is at least 8 bytes (the smallest
        // class is 16), enough to hold the `*mut u8` written into it below.
        let block = unsafe { base.add(i * block_size) };
        // SAFETY: `block` is one of the fresh, disjoint, writable
        // `block_size`-byte regions just carved out above; storing the
        // current (possibly null) free-list head as its first 8 bytes is
        // exactly how every other free block in this class is linked.
        unsafe { (block as *mut *mut u8).write(classes.heads[class]) };
        classes.heads[class] = block;
    }
    true
}

fn alloc_small(class: usize) -> *mut u8 {
    let mut classes = CLASSES.lock();
    if classes.heads[class].is_null() && !refill(&mut classes, class) {
        return ptr::null_mut();
    }
    let block = classes.heads[class];
    // SAFETY: `block` is non-null (just confirmed, or freshly refilled
    // above) and was linked into this class's free list either by
    // `refill` (a fresh, exclusively-owned slab region) or by an earlier
    // `dealloc_small` for this exact class -- either way its first 8
    // bytes hold a valid `*mut u8` (the next free block, or null).
    let next = unsafe { *(block as *const *mut u8) };
    classes.heads[class] = next;
    block
}

fn dealloc_small(ptr: *mut u8, class: usize) {
    let mut classes = CLASSES.lock();
    // SAFETY: `ptr` is a live block from this exact class -- the caller's
    // own `GlobalAlloc::dealloc` contract guarantees the same `Layout`
    // (and therefore the same class, `class_for` being a pure function of
    // it) as the matching `alloc` call; at least 8 bytes of it are free to
    // overwrite now that the caller is giving it back.
    unsafe { (ptr as *mut *mut u8).write(classes.heads[class]) };
    classes.heads[class] = ptr;
}

fn alloc_large(size: usize) -> *mut u8 {
    mem::map_anon(page_round(size)).unwrap_or(ptr::null_mut())
}

fn dealloc_large(ptr: *mut u8, size: usize) {
    // SAFETY: `ptr` was returned by this allocator's own `alloc_large` for
    // a layout of this same `size` (the caller's `GlobalAlloc::dealloc`
    // contract), which always maps exactly `page_round(size)` bytes
    // starting at `ptr` -- nothing still references it, since the caller
    // is freeing it right now.
    let _ = unsafe { mem::unmap(ptr, page_round(size)) };
}

// SAFETY: `alloc`/`dealloc` never return/require a pointer that wasn't
// produced by this exact allocator for a layout it can reconstruct the
// same class (or page-rounded size) from -- see each helper's own docs.
unsafe impl GlobalAlloc for OtterAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 || layout.align() > 4096 {
            // A zero-sized layout is never actually passed through the
            // safe `alloc`/`Box`/`Vec` APIs (the caller's own contract);
            // an alignment `map_anon` can't guarantee (it only promises 4
            // KiB) is a real, if pathological, failure -- both return null
            // rather than hand back a pointer that doesn't honour `layout`.
            return ptr::null_mut();
        }
        match class_for(layout) {
            Some(class) => alloc_small(class),
            None => alloc_large(layout.size()),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match class_for(layout) {
            Some(class) => dealloc_small(ptr, class),
            None => dealloc_large(ptr, layout.size()),
        }
    }
}
