//! Size-class (slab) allocator for small requests (brief M1-T3 step 1):
//! objects of 16, 32, 64, 128, 256, 512, 1024 or 2048 bytes, carved out of
//! 4 KiB frames obtained from the PMM and addressed through the HHDM.
//!
//! Each slab (one frame) starts with a `SlabHeader` describing its class
//! and free count, followed by the object area: a run of `class_size`-byte
//! slots, tightly packed and therefore naturally aligned to `class_size`
//! (the slab's own base is frame-aligned, i.e. a multiple of every
//! class_size here, and every slot after it is `class_size` further
//! along). Free slots form an intrusive singly linked list threaded
//! through their own memory (a `FreeNode` fits in even the smallest,
//! 16-byte class). Slabs for one class are kept on a doubly linked list
//! (`ClassState::head`) so a fully-freed slab can be unlinked in O(1) and
//! handed back to the PMM.
//!
//! `SlabHeader` also carries an `allocated` bitmap, one bit per slot
//! (kernel-review, M1-T3 fix #1): set when a slot is handed out, cleared
//! when it's freed. This is what makes double-free detection *exact*
//! rather than a best effort -- the free list alone can't tell a double
//! free apart from a legitimate one, because pushing the same node onto it
//! twice just makes the list revisit that node twice (handing the same
//! memory out to two different live callers on the next two allocations)
//! instead of failing loudly.

use core::ptr;

use super::super::addr::{FRAME_SIZE, VirtAddr};
use super::super::{hhdm, pmm};

/// The eight size classes this allocator hands out, smallest first. Every
/// entry is a power of two, so `class_index_for` can compute the class from
/// a required size with no lookup.
pub const CLASS_SIZES: [usize; 8] = [16, 32, 64, 128, 256, 512, 1024, 2048];
pub const NUM_CLASSES: usize = CLASS_SIZES.len();
/// The largest size a slab class ever satisfies; `heap::classify` routes
/// anything bigger (by size or by alignment) to the large-object path
/// instead.
pub const MAX_SLAB_SIZE: usize = 2048;

/// Upper bound on how many objects any one slab ever holds (see
/// `capacity_for`'s doc comment for why this holds for every class), and
/// therefore the width of `SlabHeader::allocated`.
const MAX_SLOTS: usize = 256;
const ALLOCATED_WORDS: usize = MAX_SLOTS / 64;

/// One free object slot, threaded through the free list of whichever slab
/// currently owns it. Lives inside the object's own (otherwise unused,
/// while free) memory, so it costs nothing extra -- every class is at
/// least 16 bytes, comfortably more than `size_of::<FreeNode>()` (8).
#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
}

/// Per-slab metadata, written into the first bytes of the frame it
/// describes. `object_area_offset` rounds its size up to a multiple of the
/// class size, so every object slot after it stays aligned to `class_size`.
#[repr(C)]
struct SlabHeader {
    class_idx: u32,
    total_count: u32,
    free_count: u32,
    free_head: *mut FreeNode,
    next: *mut SlabHeader,
    prev: *mut SlabHeader,
    /// Bit `i` set means slot `i` is currently handed out. Independent of
    /// the free list on purpose (see the module doc comment): `free_object`
    /// checks and clears this *before* touching `free_head`, so a double
    /// free is caught instead of corrupting the list.
    allocated: [u64; ALLOCATED_WORDS],
}

/// Byte offset of the first object slot within a slab of `class_size`:
/// `SlabHeader` rounded up to the next multiple of `class_size`, so it
/// never straddles a slot boundary.
fn object_area_offset(class_size: usize) -> usize {
    size_of::<SlabHeader>().div_ceil(class_size) * class_size
}

/// How many `class_size` objects fit in one 4 KiB slab after the header.
///
/// Rounding the header up to a whole `class_size` slot (see
/// `object_area_offset`) costs at most one slot per slab, which is
/// negligible for the small classes (251 of 256 for the 16-byte class) but
/// significant for the large ones: the 2048-byte class loses half its
/// slab to the header (one live object per 4 KiB slab instead of two), and
/// the 1024-byte class gets three instead of four. Simplicity over
/// density for now, since M1 has no large working set of 1-2 KiB objects
/// yet.
// TODO(later): multi-frame slabs for the 1024/2048 classes, so the header
// is amortised over more than a handful of objects.
fn capacity_for(class_size: usize) -> u32 {
    let capacity = (FRAME_SIZE - object_area_offset(class_size)) / class_size;
    debug_assert!(
        capacity <= MAX_SLOTS,
        "capacity_for: {class_size}-byte class capacity {capacity} exceeds the \
         {MAX_SLOTS}-slot `allocated` bitmap"
    );
    capacity as u32
}

/// Maps a required size (already `max(size, align)`, `1..=MAX_SLAB_SIZE`)
/// to its class index in `CLASS_SIZES`.
///
/// # Panics (debug only)
/// If `required` doesn't fit any class -- a `heap::classify` bug, since it
/// must only route requests `<= MAX_SLAB_SIZE` here.
pub fn class_index_for(required: usize) -> usize {
    let class_size = required.next_power_of_two().max(CLASS_SIZES[0]);
    debug_assert!(
        class_size <= MAX_SLAB_SIZE,
        "class_index_for: {required} doesn't fit any slab class"
    );
    (class_size.trailing_zeros() - CLASS_SIZES[0].trailing_zeros()) as usize
}

/// The doubly linked list of every live slab for one size class. `Copy`
/// (a bare pointer) purely so `[ClassState::new(); NUM_CLASSES]` can build
/// the array in `HeapState::new()` without extra ceremony.
#[derive(Clone, Copy)]
pub struct ClassState {
    head: *mut SlabHeader,
}

impl ClassState {
    pub const fn new() -> Self {
        Self { head: ptr::null_mut() }
    }
}

/// What `alloc_object` did, so the caller can keep its own `slabs` stat in
/// sync without this module needing to know about it.
pub struct AllocOutcome {
    pub ptr: *mut u8,
    pub new_slab: bool,
}

/// The slot index a live object pointer occupies within its slab, given
/// the slab's header (whose address is the slab's base) and the class
/// size. Used by both `pop_free` (to mark a slot allocated) and
/// `free_object` (to check/clear it).
fn slot_index(header: &SlabHeader, ptr: *mut u8, class_size: usize) -> usize {
    let base = ptr::from_ref(header) as usize;
    let offset = object_area_offset(class_size);
    (ptr as usize - base - offset) / class_size
}

fn slot_is_allocated(header: &SlabHeader, slot: usize) -> bool {
    header.allocated[slot / 64] & (1u64 << (slot % 64)) != 0
}

fn slot_set_allocated(header: &mut SlabHeader, slot: usize) {
    header.allocated[slot / 64] |= 1u64 << (slot % 64);
}

fn slot_clear_allocated(header: &mut SlabHeader, slot: usize) {
    header.allocated[slot / 64] &= !(1u64 << (slot % 64));
}

/// Pops one object off `header`'s free list and marks its slot allocated.
/// Callers must have just confirmed `header.free_count > 0`.
fn pop_free(header: &mut SlabHeader, class_size: usize) -> *mut u8 {
    debug_assert!(header.free_count > 0, "pop_free: slab has no free objects");
    let node = header.free_head;
    // SAFETY: `node` is non-null because `free_count > 0`, and every
    // pointer ever linked into a slab's free list (by this function's
    // caller, or by the initial slab setup in `alloc_object`) points at a
    // valid, currently-free object slot inside this same slab.
    header.free_head = unsafe { (*node).next };
    header.free_count -= 1;
    let slot = slot_index(header, node as *mut u8, class_size);
    slot_set_allocated(header, slot);
    node as *mut u8
}

/// Removes `header` from `class`'s list by re-linking its neighbours --
/// called only once it has no live objects left.
fn unlink(class: &mut ClassState, header: &SlabHeader) {
    if header.prev.is_null() {
        class.head = header.next;
    } else {
        // SAFETY: `header.prev` is a live `SlabHeader` this same list
        // manages (only ever set to another node already on the list, or
        // null); nothing outside this module ever touches it.
        unsafe { (*header.prev).next = header.next };
    }
    if !header.next.is_null() {
        // SAFETY: same reasoning as `header.prev` above, for the forward link.
        unsafe { (*header.next).prev = header.prev };
    }
}

/// Allocates one `class_idx`-sized object, reusing a slab with spare
/// capacity if one exists, otherwise pulling a fresh frame from the PMM.
/// `None` if the PMM is out of memory.
pub fn alloc_object(class: &mut ClassState, class_idx: usize) -> Option<AllocOutcome> {
    let class_size = CLASS_SIZES[class_idx];

    let mut cursor = class.head;
    while !cursor.is_null() {
        // SAFETY: every pointer reachable from `class.head` was written by
        // this module (either just below, or by an earlier call), points
        // at a `SlabHeader` occupying the start of a frame this allocator
        // owns exclusively, and every access to it happens with
        // `heap::HEAP`'s lock held, so no other reference exists.
        let header = unsafe { &mut *cursor };
        if header.free_count > 0 {
            let ptr = pop_free(header, class_size);
            return Some(AllocOutcome { ptr, new_slab: false });
        }
        cursor = header.next;
    }

    // No slab has room: carve a fresh one out of a new frame.
    let phys = pmm::alloc_frame()?;
    let base = hhdm::phys_to_virt(phys).as_u64() as *mut u8;
    let header_ptr = base as *mut SlabHeader;
    let capacity = capacity_for(class_size);
    let offset = object_area_offset(class_size);

    // SAFETY: `phys` was just allocated by `pmm::alloc_frame`, so its HHDM
    // alias is at least `FRAME_SIZE` bytes, exclusively ours to initialise,
    // and 4096-byte aligned (frame granularity) -- more than enough for
    // `SlabHeader`'s size and alignment.
    unsafe {
        header_ptr.write(SlabHeader {
            class_idx: class_idx as u32,
            total_count: capacity,
            free_count: capacity,
            free_head: ptr::null_mut(),
            next: class.head,
            prev: ptr::null_mut(),
            allocated: [0; ALLOCATED_WORDS],
        });
    }
    if !class.head.is_null() {
        // SAFETY: `class.head` (about to become this new slab's `next`)
        // was linked in the same way by an earlier call, so it's a live
        // `SlabHeader` this module may update.
        unsafe { (*class.head).prev = header_ptr };
    }
    class.head = header_ptr;

    // Thread every slot onto the fresh slab's free list.
    for i in (0..capacity as usize).rev() {
        // SAFETY: `offset + i * class_size + class_size <= FRAME_SIZE` by
        // `capacity_for`'s construction, so `obj` lies entirely inside the
        // frame just initialised above, which this allocator exclusively
        // owns.
        let obj = unsafe { base.add(offset + i * class_size) } as *mut FreeNode;
        // SAFETY: `obj` is `class_size`-aligned object memory inside the
        // slab just initialised above; `class_size >= 16` comfortably fits
        // a `FreeNode` (8 bytes, 8-byte aligned).
        unsafe {
            (*obj).next = (*header_ptr).free_head;
            (*header_ptr).free_head = obj;
        }
    }

    // SAFETY: `header_ptr` was just written above and is exclusively ours.
    let header = unsafe { &mut *header_ptr };
    let ptr = pop_free(header, class_size);
    Some(AllocOutcome { ptr, new_slab: true })
}

/// Frees an object previously returned by `alloc_object`. `expected_class_idx`
/// is the class `heap::classify` computed from the `Layout` passed to
/// `dealloc` -- it must match the class the object was actually allocated
/// with (`header.class_idx`), and the object's slot must currently be
/// marked allocated; both are real, always-on `assert!`s (kernel-review,
/// M1-T3 fix #1: caller bugs here are exactly as serious as the ones
/// `mm::pmm::free_frame` already refuses to let slide).
///
/// Returns whether the slab this object lived in became completely free
/// (and was therefore unlinked and handed back to the PMM) -- the caller
/// uses this to keep its own `slabs` stat in sync.
///
/// # Panics
/// If `expected_class_idx` doesn't match the class this object was
/// actually allocated with ("class mismatch"), or if its slot is already
/// marked free ("double free").
pub fn free_object(class: &mut ClassState, ptr: *mut u8, expected_class_idx: usize) -> bool {
    let slab_base = (ptr as usize) & !(FRAME_SIZE - 1);
    let header_ptr = slab_base as *mut SlabHeader;
    // SAFETY: every pointer this allocator hands out via `alloc_object`
    // lies inside the object area of a slab frame whose base (`ptr`
    // rounded down to 4096) holds the `SlabHeader` written there by
    // `alloc_object`; the caller (`HeapState::dealloc`) only ever calls
    // this with such a pointer, under the same lock.
    let header = unsafe { &mut *header_ptr };

    assert_eq!(
        header.class_idx as usize, expected_class_idx,
        "heap: class mismatch freeing 0x{:x}: allocated as the {}-byte class, freed as the {}-byte class",
        ptr as usize,
        CLASS_SIZES[header.class_idx as usize],
        CLASS_SIZES[expected_class_idx]
    );

    let class_size = CLASS_SIZES[expected_class_idx];
    let slot = slot_index(header, ptr, class_size);
    assert!(slot_is_allocated(header, slot), "heap: double free at 0x{:x}", ptr as usize);
    slot_clear_allocated(header, slot);

    let node = ptr as *mut FreeNode;
    // SAFETY: `ptr` is `class_size`-aligned object memory this allocator
    // owns again now that the caller is freeing it; every class is at
    // least 16 bytes, comfortably fitting a `FreeNode`.
    unsafe { (*node).next = header.free_head };
    header.free_head = node;
    header.free_count += 1;

    if header.free_count != header.total_count {
        return false;
    }

    unlink(class, header);
    // SAFETY: this frame was allocated by `pmm::alloc_frame` in
    // `alloc_object` and, per the check above, now has zero live objects
    // in it and has just been unlinked from `class`'s list, so nothing
    // still refers to it; converting its HHDM base back to a physical
    // address and freeing it is exactly `pmm::free_frame`'s contract.
    let phys = hhdm::virt_to_phys_hhdm(VirtAddr::new(header_ptr as u64));
    pmm::free_frame(phys);
    true
}
