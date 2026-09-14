//! The physical memory manager (brief M1-T2 step 4): a single, global,
//! frame-granularity bitmap built from the Limine memory map, handing out
//! and reclaiming 4 KiB physical frames for the rest of the kernel
//! (DECISIONS.md D2: our own allocator, no crate does this for us).

use limine::memmap::{self, Entry};
use spin::Mutex;

use super::addr::{FRAME_SIZE, PhysAddr};
use super::bitmap::FrameBitmap;
use super::hhdm;
use crate::kprintln;

/// Enough slots for every USABLE entry the Limine memory map reports. Real
/// firmware maps and QEMU alike report a handful of these (rarely more
/// than a dozen); this is generous headroom, not a tight fit. If it's ever
/// exceeded, `init` logs a warning and `for_each_usable_region` simply
/// skips the overflow -- the bitmap itself (built directly from the full
/// entry list, not from this table) is unaffected either way.
const MAX_USABLE_REGIONS: usize = 64;

/// Frame counts, in frames (multiply by `FRAME_SIZE` for bytes).
pub struct PmmStats {
    pub total: usize,
    pub used: usize,
    pub free: usize,
}

struct Pmm {
    bitmap: FrameBitmap<'static>,
    /// Where the bitmap itself lives in physical memory, so callers (and
    /// `bitmap_region`, used by tests) can tell an ordinary allocation
    /// apart from the bitmap's own backing storage.
    bitmap_base: PhysAddr,
    bitmap_bytes: usize,
    usable_regions: [(u64, u64); MAX_USABLE_REGIONS],
    usable_region_count: usize,
}

/// The one PMM instance. `None` until `init` runs; every accessor below
/// panics with a clear message if called before that (a programmer error,
/// not a runtime condition -- nothing should be allocating memory before
/// `mm::init`).
static PMM: Mutex<Option<Pmm>> = Mutex::new(None);

fn with_pmm<R>(f: impl FnOnce(&mut Pmm) -> R) -> R {
    let mut guard = PMM.lock();
    let pmm = guard.as_mut().expect("mm::pmm::init was never called");
    f(pmm)
}

fn with_pmm_ref<R>(f: impl FnOnce(&Pmm) -> R) -> R {
    let guard = PMM.lock();
    let pmm = guard.as_ref().expect("mm::pmm::init was never called");
    f(pmm)
}

/// A short mnemonic for a Limine memory-map entry type, for the boot log.
fn type_name(type_: u64) -> &'static str {
    match type_ {
        memmap::MEMMAP_USABLE => "USABLE",
        memmap::MEMMAP_RESERVED => "RESERVED",
        memmap::MEMMAP_ACPI_RECLAIMABLE => "ACPI_RECLAIMABLE",
        memmap::MEMMAP_ACPI_NVS => "ACPI_NVS",
        memmap::MEMMAP_BAD_MEMORY => "BAD_MEMORY",
        memmap::MEMMAP_BOOTLOADER_RECLAIMABLE => "BOOTLOADER_RECLAIMABLE",
        memmap::MEMMAP_EXECUTABLE_AND_MODULES => "EXECUTABLE_AND_MODULES",
        memmap::MEMMAP_FRAMEBUFFER => "FRAMEBUFFER",
        memmap::MEMMAP_MAPPED_RESERVED => "MAPPED_RESERVED",
        _ => "UNKNOWN",
    }
}

/// Builds the frame bitmap from `entries` (the Limine memory map) and
/// installs it as the global PMM. `mm::init` calls this once, after
/// `hhdm::init`.
///
/// Bootloader-reclaimable regions (Limine's own structures, our boot
/// stack) are deliberately left reserved rather than folded into the
/// usable set: reclaiming them safely means nothing can still be relying
/// on Limine-owned data, which isn't guaranteed this early.
// TODO(M1-T4): reclaim bootloader-reclaimable regions once the VMM/heap no
// longer need anything Limine handed us.
pub fn init(entries: &[&Entry]) {
    let mut usable_bytes: u64 = 0;
    let mut usable_region_log_count: usize = 0;
    let mut highest_usable_end: u64 = 0;

    for entry in entries {
        let end = entry.base + entry.length;
        kprintln!("[pmm] 0x{:x}-0x{:x} {}", entry.base, end, type_name(entry.type_));
        if entry.type_ == memmap::MEMMAP_USABLE {
            usable_bytes += entry.length;
            usable_region_log_count += 1;
            highest_usable_end = highest_usable_end.max(end);
        }
    }

    let total_frames = (highest_usable_end / FRAME_SIZE as u64) as usize;
    assert!(total_frames > 0, "mm::pmm::init: no USABLE memory reported by Limine");

    let bitmap_words = total_frames.div_ceil(64);
    let bitmap_bytes = bitmap_words * 8;

    let bitmap_region = entries
        .iter()
        .find(|e| e.type_ == memmap::MEMMAP_USABLE && (e.length as usize) >= bitmap_bytes)
        .expect("mm::pmm::init: no single USABLE region is large enough to hold the frame bitmap");

    let bitmap_base = PhysAddr::new(bitmap_region.base);
    let bitmap_virt = hhdm::phys_to_virt(bitmap_base);

    // SAFETY: `bitmap_region` is a USABLE region at least `bitmap_bytes`
    // bytes long (just confirmed above), so the `bitmap_words` consecutive
    // `u64`s starting at its HHDM alias lie entirely inside RAM Limine
    // described to us and entirely inside this one region -- no other
    // owner exists yet (this runs once, before `PMM` is populated,
    // single-threaded, interrupts off, and nothing else has a pointer into
    // this region). Limine memory-map entries are page-aligned, so
    // `bitmap_base`'s address (and therefore `bitmap_virt`'s) is at least
    // 4096-byte, hence 8-byte, aligned -- valid for a `[u64]`. Every bit
    // pattern is a valid `u64`, so handing out this slice before anything
    // has written to it is sound; `fill_used` immediately below is the
    // first thing that reads or writes it.
    let words: &'static mut [u64] =
        unsafe { core::slice::from_raw_parts_mut(bitmap_virt.as_u64() as *mut u64, bitmap_words) };

    let mut bitmap = FrameBitmap::new(words, 0, total_frames);

    // Default to "used" (i.e. "not ours to hand out"): reserved regions,
    // ACPI tables, gaps the memory map never mentions, and anything else
    // that isn't explicitly USABLE all stay used by simply never being
    // freed below.
    bitmap.fill_used();

    for entry in entries.iter().filter(|e| e.type_ == memmap::MEMMAP_USABLE) {
        // Limine guarantees USABLE entries are page-aligned, but round
        // conservatively (up at the start, down at the end) instead of
        // trusting that: a partial frame at either edge is memory this
        // entry doesn't actually promise is fully usable.
        let start_frame = entry.base.div_ceil(FRAME_SIZE as u64);
        let end_frame = (entry.base + entry.length) / FRAME_SIZE as u64;
        for frame in start_frame..end_frame {
            bitmap.set_free(frame);
        }
    }

    // The bitmap's own storage sits inside the USABLE region we just
    // marked free above -- claim it back before anyone can allocate it out
    // from under us.
    let bitmap_frame_count = bitmap_bytes.div_ceil(FRAME_SIZE) as u64;
    let bitmap_start_frame = bitmap_base.frame_index();
    for frame in bitmap_start_frame..bitmap_start_frame + bitmap_frame_count {
        bitmap.set_used(frame);
    }

    // Never hand out physical page 0: plenty of real firmware/BIOS data
    // structures live there in practice, and it also makes a physical
    // address of 0 a reliable "this was never allocated" sentinel.
    bitmap.set_used(0);

    let mut usable_regions = [(0u64, 0u64); MAX_USABLE_REGIONS];
    let mut usable_region_count = 0usize;
    for entry in entries.iter().filter(|e| e.type_ == memmap::MEMMAP_USABLE) {
        if usable_region_count == MAX_USABLE_REGIONS {
            kprintln!(
                "[pmm] WARNING: more than {MAX_USABLE_REGIONS} USABLE regions; \
                 for_each_usable_region will skip the rest (the bitmap itself is unaffected)"
            );
            break;
        }
        usable_regions[usable_region_count] = (entry.base, entry.length);
        usable_region_count += 1;
    }

    let free_frames = bitmap.count_free();

    *PMM.lock() = Some(Pmm { bitmap, bitmap_base, bitmap_bytes, usable_regions, usable_region_count });

    kprintln!(
        "[pmm] usable: {} MiB in {} regions; bitmap {} KiB at 0x{:x}; free frames: {}",
        usable_bytes / (1024 * 1024),
        usable_region_log_count,
        bitmap_bytes / 1024,
        bitmap_base.as_u64(),
        free_frames
    );
}

/// Current frame counts. Panics if called before `init`.
pub fn stats() -> PmmStats {
    with_pmm_ref(|pmm| {
        let total = pmm.bitmap.frame_count();
        let free = pmm.bitmap.count_free();
        PmmStats { total, used: total - free, free }
    })
}

/// Allocates one free 4 KiB frame, or `None` if RAM is exhausted.
pub fn alloc_frame() -> Option<PhysAddr> {
    with_pmm(|pmm| {
        let frame = pmm.bitmap.find_free_run(1, 1)?;
        pmm.bitmap.set_used(frame);
        Some(PhysAddr::new(frame * FRAME_SIZE as u64))
    })
}

/// Allocates one free 4 KiB frame and zeroes it (read back through the
/// HHDM), or `None` if RAM is exhausted.
pub fn alloc_frame_zeroed() -> Option<PhysAddr> {
    let phys = alloc_frame()?;
    let virt = hhdm::phys_to_virt(phys);
    // SAFETY: `phys` was just allocated by `alloc_frame` above, so nothing
    // else can be holding a reference to it yet (single-threaded, and no
    // other caller has learned this address). The HHDM maps every physical
    // frame the bitmap covers 1:1 at a fixed offset, so `virt`'s address is
    // valid and writable for exactly `FRAME_SIZE` bytes here.
    unsafe { core::ptr::write_bytes(virt.as_u64() as *mut u8, 0u8, FRAME_SIZE) };
    Some(phys)
}

/// Allocates `n_frames` contiguous frames whose base is aligned to
/// `align_frames` frames, or `None` if no such run exists.
pub fn alloc_contiguous(n_frames: usize, align_frames: u64) -> Option<PhysAddr> {
    if n_frames == 0 {
        return None;
    }
    with_pmm(|pmm| {
        let start = pmm.bitmap.find_free_run(n_frames, align_frames)?;
        for frame in start..start + n_frames as u64 {
            pmm.bitmap.set_used(frame);
        }
        Some(PhysAddr::new(start * FRAME_SIZE as u64))
    })
}

/// Frees one frame previously returned by `alloc_frame`/`alloc_frame_zeroed`
/// or by `alloc_contiguous`/`free_contiguous`'s bookkeeping.
///
/// # Panics
/// If `phys` is not frame-aligned, is frame 0 (never allocated in the
/// first place), lies outside the RAM the PMM was built from, or is
/// already free -- each of those is a kernel bug, not a runtime condition,
/// so this panics in debug and release builds alike rather than silently
/// corrupting the free list.
pub fn free_frame(phys: PhysAddr) {
    assert!(
        phys.is_aligned(FRAME_SIZE as u64),
        "free_frame: 0x{:x} is not frame-aligned",
        phys.as_u64()
    );
    let frame = phys.frame_index();
    assert_ne!(frame, 0, "free_frame: physical frame 0 is never allocated, so it can't be freed");

    with_pmm(|pmm| {
        assert!(
            frame < pmm.bitmap.frame_count() as u64,
            "free_frame: 0x{:x} is outside the RAM this PMM manages",
            phys.as_u64()
        );
        assert!(
            pmm.bitmap.is_used(frame),
            "free_frame: double free (or freeing a never-allocated frame) at 0x{:x}",
            phys.as_u64()
        );
        pmm.bitmap.set_free(frame);
    });
}

/// Frees `n_frames` contiguous frames starting at `phys` (the counterpart
/// to `alloc_contiguous`), one `free_frame` call at a time -- see that
/// function's panic conditions, which apply per-frame here too.
pub fn free_contiguous(phys: PhysAddr, n_frames: usize) {
    assert!(
        phys.is_aligned(FRAME_SIZE as u64),
        "free_contiguous: 0x{:x} is not frame-aligned",
        phys.as_u64()
    );
    let base_frame = phys.frame_index();
    for i in 0..n_frames as u64 {
        free_frame(PhysAddr::new((base_frame + i) * FRAME_SIZE as u64));
    }
}

/// Calls `f(base, length_bytes)` once for every region the Limine memory
/// map reported as USABLE at `init` time (not "currently free" -- frames
/// inside a usable region may well be allocated by now). Useful for
/// callers (the VMM, M1-T4) that need to walk RAM by region rather than by
/// frame.
pub fn for_each_usable_region<F: FnMut(PhysAddr, u64)>(mut f: F) {
    with_pmm_ref(|pmm| {
        for &(base, len) in &pmm.usable_regions[..pmm.usable_region_count] {
            f(PhysAddr::new(base), len);
        }
    });
}

/// Whether `phys`'s frame is currently marked used -- includes frames that
/// were never USABLE, the bitmap's own frames, and frame 0, not just ones
/// handed out by `alloc_frame`. Mainly for tests/debugging.
pub fn is_frame_used(phys: PhysAddr) -> bool {
    with_pmm_ref(|pmm| pmm.bitmap.is_used(phys.frame_index()))
}

/// The physical range `[base, base + bytes)` occupied by the bitmap's own
/// backing storage, so callers (tests, in particular) can confirm an
/// allocation never overlaps it.
pub fn bitmap_region() -> (PhysAddr, usize) {
    with_pmm_ref(|pmm| (pmm.bitmap_base, pmm.bitmap_bytes))
}
