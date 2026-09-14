//! The physical memory manager (brief M1-T2 step 4, hardened per
//! kernel-review): a single, global, frame-granularity bitmap built from
//! the Limine memory map, handing out and reclaiming 4 KiB physical
//! frames for the rest of the kernel (DECISIONS.md D2: our own allocator,
//! no crate does this for us).

use limine::memmap::{self, Entry};
use spin::Mutex;

use super::addr::{FRAME_SIZE, PhysAddr};
use super::bitmap::FrameBitmap;
use super::hhdm;
use crate::arch::x86_64::interrupts::without_interrupts;
use crate::kprintln;

/// Enough slots for every USABLE entry the Limine memory map reports. Real
/// firmware maps and QEMU alike report a handful of these (rarely more
/// than a dozen); this is generous headroom, not a tight fit. If it's ever
/// exceeded, `init` logs a warning and `for_each_usable_region` simply
/// skips the overflow -- the bitmap itself (built directly from the full
/// entry list, not from this table) is unaffected either way.
const MAX_USABLE_REGIONS: usize = 64;

/// Real-mode-addressable memory (kernel-review, M1-T2 fix #2): the future
/// SMP real-mode trampoline has to live below here, so these frames are
/// carved out of the general pool at `init` and handed out only through
/// `alloc_frame_low`.
const LOW_MEM_LIMIT: u64 = 0x10_0000;
const LOW_MEM_FRAME_COUNT: usize = (LOW_MEM_LIMIT / FRAME_SIZE as u64) as usize;
const LOW_BITMAP_WORD_COUNT: usize = LOW_MEM_FRAME_COUNT.div_ceil(64);

/// Backing storage for `Pmm::low_bitmap`. A fixed-size kernel static
/// (32 bytes for the default 1 MiB / 4 KiB = 256 frames) rather than a
/// frame carved out of RAM: it's tiny, and needed before any concept of
/// "a spare frame" exists.
static mut LOW_BITMAP_WORDS: [u64; LOW_BITMAP_WORD_COUNT] = [0; LOW_BITMAP_WORD_COUNT];

/// Frame counts, in frames (multiply by `FRAME_SIZE` for bytes).
pub struct PmmStats {
    pub total: usize,
    pub used: usize,
    pub free: usize,
    /// How many of `total`'s frames are below 1 MiB and therefore
    /// reserved out of this general pool (see `LOW_MEM_LIMIT`) rather than
    /// simply allocated -- a subset of `used`, broken out separately
    /// (kernel-review, M1-T2 fix #2).
    pub low_reserved: usize,
}

struct Pmm {
    bitmap: FrameBitmap<'static>,
    /// Where the bitmap itself lives in physical memory, so `is_usable_frame`
    /// can refuse to ever free it back, and so tests (`bitmap_region`) can
    /// tell an ordinary allocation apart from the bitmap's own storage.
    bitmap_base: PhysAddr,
    bitmap_bytes: usize,
    /// The dedicated pool for `alloc_frame_low`: frames `[0,
    /// low_bitmap.frame_count())`, i.e. below `LOW_MEM_LIMIT`, tracked
    /// independently of `bitmap` (which has all of them permanently
    /// marked used instead -- see `init`).
    low_bitmap: FrameBitmap<'static>,
    low_reserved: usize,
    usable_regions: [(u64, u64); MAX_USABLE_REGIONS],
    usable_region_count: usize,
}

impl Pmm {
    /// Whether `frame` lies inside some USABLE region the Limine memory
    /// map reported, *and* isn't part of the frame bitmap's own backing
    /// storage (kernel-review, M1-T2 fix #1). Both `bitmap` and
    /// `low_bitmap` mark plenty of frames "used" that were never handed
    /// out by any `alloc_*` call -- reserved regions, ACPI tables,
    /// bootloader-reclaimable/kernel-image frames, unlisted gaps, and the
    /// bitmap's own metadata -- and there is no spare bit to tell those
    /// apart from a real allocation; this table is that distinction, and
    /// `free_frame` consults it before touching either bitmap at all.
    fn is_usable_frame(&self, frame: u64) -> bool {
        let addr = frame * FRAME_SIZE as u64;
        let bitmap_start = self.bitmap_base.as_u64();
        let bitmap_end = bitmap_start + self.bitmap_bytes as u64;
        if addr >= bitmap_start && addr < bitmap_end {
            return false;
        }
        self.usable_regions[..self.usable_region_count]
            .iter()
            .any(|&(base, len)| addr >= base && addr + FRAME_SIZE as u64 <= base + len)
    }

    /// The bitmap that owns `frame`'s used/free bit: the dedicated
    /// low-memory pool for frames below `LOW_MEM_LIMIT`, or the general
    /// one for everything else.
    fn bitmap_for(&mut self, frame: u64) -> &mut FrameBitmap<'static> {
        if frame < self.low_bitmap.frame_count() as u64 {
            &mut self.low_bitmap
        } else {
            &mut self.bitmap
        }
    }
}

/// The one PMM instance. `None` until `init` runs; every accessor below
/// panics with a clear message if called before that (a programmer error,
/// not a runtime condition -- nothing should be allocating memory before
/// `mm::init`).
static PMM: Mutex<Option<Pmm>> = Mutex::new(None);

/// Locks `PMM` for mutation with interrupts disabled (kernel-review,
/// M1-T2 fix #6): `spin::Mutex` isn't reentrant, so a future interrupt
/// handler that ran while this lock was already held on the same core
/// would deadlock forever instead of just blocking. Interrupts aren't
/// enabled anywhere yet (M1 keeps `cli` for the whole milestone), so this
/// costs nothing today; it exists so the PMM is already safe to call from
/// IRQ context once a later task turns interrupts on.
fn with_pmm<R>(f: impl FnOnce(&mut Pmm) -> R) -> R {
    without_interrupts(|| {
        let mut guard = PMM.lock();
        let pmm = guard.as_mut().expect("mm::pmm::init was never called");
        f(pmm)
    })
}

/// Read-only counterpart to `with_pmm`; see its docs.
fn with_pmm_ref<R>(f: impl FnOnce(&Pmm) -> R) -> R {
    without_interrupts(|| {
        let guard = PMM.lock();
        let pmm = guard.as_ref().expect("mm::pmm::init was never called");
        f(pmm)
    })
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

    // Never place the bitmap below 1 MiB (kernel-review, M1-T2 fix #2):
    // pick the *largest* USABLE region at or above that line which is
    // big enough to hold it. Scanning for the maximum `length` among the
    // candidates that already satisfy the size requirement is exactly
    // "the largest one, falling back to the largest one that still fits"
    // in a single pass -- a smaller region can never be "the largest that
    // fits" while a bigger one already does.
    let bitmap_region = entries
        .iter()
        .filter(|e| e.type_ == memmap::MEMMAP_USABLE)
        .filter(|e| e.base >= LOW_MEM_LIMIT)
        .filter(|e| (e.length as usize) >= bitmap_bytes)
        .max_by_key(|e| e.length)
        .expect("mm::pmm::init: no USABLE region at or above 1 MiB is large enough for the bitmap");

    let bitmap_base = PhysAddr::new(bitmap_region.base);
    let bitmap_virt = hhdm::phys_to_virt(bitmap_base);

    // SAFETY: `bitmap_region` is a USABLE region at or above 1 MiB, at
    // least `bitmap_bytes` bytes long (just confirmed above), so the
    // `bitmap_words` consecutive `u64`s starting at its HHDM alias lie
    // entirely inside RAM Limine described to us and entirely inside this
    // one region -- no other owner exists yet (this runs once, before
    // `PMM` is populated, single-threaded, interrupts off, and nothing
    // else has a pointer into this region). Limine memory-map entries are
    // page-aligned, so `bitmap_base`'s address (and therefore
    // `bitmap_virt`'s) is at least 4096-byte, hence 8-byte, aligned --
    // valid for a `[u64]`. Every bit pattern is a valid `u64`, so handing
    // out this slice before anything has written to it is sound;
    // `fill_used` immediately below is the first thing that reads or
    // writes it.
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

    // Reserve everything below 1 MiB out of the general pool (kernel-
    // review, M1-T2 fix #2) and build a separate free-list for it instead
    // (`alloc_frame_low`). `low_bitmap` only tracks frames Limine actually
    // called USABLE down there -- anything else (reserved/ACPI/gaps below
    // 1 MiB) stays used in *both* bitmaps, same as it always was.
    let low_frame_count = LOW_MEM_FRAME_COUNT.min(total_frames);
    // SAFETY: `LOW_BITMAP_WORDS` is a `'static` array; nothing else has a
    // pointer into it yet (this runs once, before `PMM` is populated,
    // single-threaded, interrupts off). Taking the pointer via `&raw mut`
    // (never an intermediate `&mut` to the `static mut` itself) sidesteps
    // both the `static_mut_refs` lint and the "this is never mutated
    // through a real `&mut`" rodata-folding hazard `gdt.rs` documents for
    // the identical pattern (`TSS`/`GDT`/`IDT`).
    let low_words: &'static mut [u64] = unsafe {
        core::slice::from_raw_parts_mut(
            (&raw mut LOW_BITMAP_WORDS).cast::<u64>(),
            LOW_BITMAP_WORD_COUNT,
        )
    };
    let mut low_bitmap = FrameBitmap::new(low_words, 0, low_frame_count);
    low_bitmap.fill_used();

    let mut low_reserved = 0usize;
    for entry in entries.iter().filter(|e| e.type_ == memmap::MEMMAP_USABLE) {
        let start_frame = entry.base.div_ceil(FRAME_SIZE as u64);
        let end_frame = ((entry.base + entry.length) / FRAME_SIZE as u64).min(low_frame_count as u64);
        if start_frame >= end_frame {
            continue;
        }
        for frame in start_frame..end_frame {
            bitmap.set_used(frame); // pulled out of the general pool...
            low_bitmap.set_free(frame); // ...and into the low pool instead.
            low_reserved += 1;
        }
    }

    // Never hand out physical page 0, from either pool: plenty of real
    // firmware/BIOS data structures live there in practice, and it also
    // makes a physical address of 0 a reliable "this was never allocated"
    // sentinel.
    bitmap.set_used(0);
    low_bitmap.set_used(0);

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

    *PMM.lock() = Some(Pmm {
        bitmap,
        bitmap_base,
        bitmap_bytes,
        low_bitmap,
        low_reserved,
        usable_regions,
        usable_region_count,
    });

    kprintln!(
        "[pmm] usable: {} MiB in {} regions; bitmap {} KiB at 0x{:x}; free frames: {}",
        usable_bytes / (1024 * 1024),
        usable_region_log_count,
        bitmap_bytes / 1024,
        bitmap_base.as_u64(),
        free_frames
    );
    kprintln!("[pmm] low memory reserved: {low_reserved} frames");
}

/// Current frame counts. Panics if called before `init`.
pub fn stats() -> PmmStats {
    with_pmm_ref(|pmm| {
        let total = pmm.bitmap.frame_count();
        let free = pmm.bitmap.count_free();
        PmmStats { total, used: total - free, free, low_reserved: pmm.low_reserved }
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

/// Allocates one free frame from below 1 MiB (kernel-review, M1-T2 fix
/// #2): needed later for the SMP real-mode trampoline, which must live in
/// real-mode-addressable memory. Entirely separate from the general pool
/// -- see `LOW_MEM_LIMIT` and `Pmm::low_bitmap`.
pub fn alloc_frame_low() -> Option<PhysAddr> {
    with_pmm(|pmm| {
        let frame = pmm.low_bitmap.find_free_run(1, 1)?;
        pmm.low_bitmap.set_used(frame);
        Some(PhysAddr::new(frame * FRAME_SIZE as u64))
    })
}

/// Frees one frame previously returned by `alloc_frame`/`alloc_frame_zeroed`,
/// `alloc_contiguous`/`free_contiguous`'s bookkeeping, or `alloc_frame_low`.
///
/// # Panics
/// If `phys` is not frame-aligned, is frame 0 (never allocated in the
/// first place), is not a usable frame this PMM could ever have handed out
/// (reserved/ACPI/bootloader-reclaimable/kernel-image memory, the frame
/// bitmap's own storage, or an address outside all known RAM), or is
/// already free (a double free) -- each of those is a kernel bug, not a
/// runtime condition, so this panics in debug and release builds alike
/// rather than silently corrupting a free list (kernel-review, M1-T2 fix
/// #1).
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
            pmm.is_usable_frame(frame),
            "free_frame: 0x{:x} is not a usable frame (reserved, ACPI, bootloader-reclaimable, \
             kernel-image, and frame-bitmap memory are never ours to free)",
            phys.as_u64()
        );
        let bmp = pmm.bitmap_for(frame);
        assert!(bmp.is_used(frame), "free_frame: double free at 0x{:x}", phys.as_u64());
        bmp.set_free(frame);
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
/// inside a usable region may well be allocated by now, and this includes
/// regions below 1 MiB). Useful for callers (the VMM, M1-T4) that need to
/// walk RAM by region rather than by frame.
pub fn for_each_usable_region<F: FnMut(PhysAddr, u64)>(mut f: F) {
    with_pmm_ref(|pmm| {
        for &(base, len) in &pmm.usable_regions[..pmm.usable_region_count] {
            f(PhysAddr::new(base), len);
        }
    });
}

/// Whether `phys`'s frame is currently marked used, in whichever bitmap
/// (general or low-memory) actually owns it -- includes frames that were
/// never USABLE, the bitmap's own frames, and frame 0, not just ones
/// handed out by `alloc_frame`/`alloc_frame_low`. Mainly for
/// tests/debugging.
pub fn is_frame_used(phys: PhysAddr) -> bool {
    with_pmm_ref(|pmm| {
        let frame = phys.frame_index();
        if frame < pmm.low_bitmap.frame_count() as u64 {
            pmm.low_bitmap.is_used(frame)
        } else {
            pmm.bitmap.is_used(frame)
        }
    })
}

/// The physical range `[base, base + bytes)` occupied by the bitmap's own
/// backing storage, so callers (tests, in particular) can confirm an
/// allocation never overlaps it.
pub fn bitmap_region() -> (PhysAddr, usize) {
    with_pmm_ref(|pmm| (pmm.bitmap_base, pmm.bitmap_bytes))
}
