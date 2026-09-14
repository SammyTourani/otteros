//! `mm::addr`/`mm::bitmap`/`mm::pmm` tests (brief M1-T2), moved here
//! verbatim from `test_main.rs`'s original inline `mod test_cases` (brief
//! M1-T3, which needed the room).
//!
//! The `pmm` tests share the one real, global PMM `mm::init()` already
//! built from the actual Limine memory map (see lib.rs's `init()`), so
//! every test below frees exactly what it allocates: leaving a frame
//! permanently allocated would shift every later test's view of "the
//! lowest free frame", not just its own.

use otteros_kernel::mm::addr::{FRAME_SIZE, PhysAddr, VirtAddr};
use otteros_kernel::mm::bitmap::FrameBitmap;
use otteros_kernel::mm::{hhdm, pmm};

// --- mm::addr (brief M1-T2 step 1) ----------------------------------

/// `align_down`/`align_up`/`is_aligned`/`frame_index` on both address
/// newtypes -- exercised directly here since `pmm`'s own tests below
/// only exercise them indirectly (via real allocations).
#[test_case]
fn addr_alignment_helpers() {
    let a = PhysAddr::new(0x1234);
    assert_eq!(a.align_down(0x1000).as_u64(), 0x1000);
    assert_eq!(a.align_up(0x1000).as_u64(), 0x2000);
    assert!(!a.is_aligned(0x1000));
    assert!(PhysAddr::new(0x2000).is_aligned(0x1000));
    assert_eq!(PhysAddr::new(0x3000).frame_index(), 3);

    let v = VirtAddr::new(0xffff_8000_0000_1234);
    assert_eq!(v.align_down(0x1000).as_u64(), 0xffff_8000_0000_1000);
    assert_eq!(v.align_up(0x1000).as_u64(), 0xffff_8000_0000_2000);
}

// --- mm::bitmap (brief M1-T2 step 3) ---------------------------------
//
// `FrameBitmap` is pure logic over caller-supplied storage, so these
// exercise it directly over small on-stack arrays -- independent of
// `pmm`'s real, RAM-sized bitmap tested below.

#[test_case]
fn bitmap_set_free_used_count() {
    let mut words = [0u64; 4]; // 256 frames
    let mut bmp = FrameBitmap::new(&mut words, 0, 256);
    bmp.fill_free();
    assert_eq!(bmp.count_free(), 256);

    bmp.set_used(10);
    assert!(bmp.is_used(10));
    assert_eq!(bmp.count_free(), 255);

    bmp.set_free(10);
    assert!(!bmp.is_used(10));
    assert_eq!(bmp.count_free(), 256);

    bmp.fill_used();
    assert_eq!(bmp.count_free(), 0);
    assert!(bmp.is_used(0));
    assert!(bmp.is_used(255));
}

#[test_case]
fn bitmap_find_free_run_respects_alignment() {
    let mut words = [0u64; 2]; // 128 frames
    let mut bmp = FrameBitmap::new(&mut words, 0, 128);
    bmp.fill_used();
    for frame in 64..72 {
        bmp.set_free(frame);
    }

    // Exactly 8 free frames [64, 72); 8-frame-aligned start -> 64.
    assert_eq!(bmp.find_free_run(8, 8), Some(64));
    // Only 8 free frames total: asking for 9 must fail.
    assert_eq!(bmp.find_free_run(9, 1), None);
    // No start aligned to 5 has 8 consecutive free frames after it.
    assert_eq!(bmp.find_free_run(8, 5), None);
}

#[test_case]
fn bitmap_find_free_run_none_when_full() {
    let mut words = [u64::MAX; 4]; // 256 frames, all used
    let bmp = FrameBitmap::new(&mut words, 0, 256);
    assert_eq!(bmp.find_free_run(1, 1), None);
}

/// `find_free_run` over a free range that straddles a 64-bit word
/// boundary (frames 60..70 span word 0's top and word 1's bottom,
/// since each word covers 64 frames) -- exercises `first_used_rel`'s
/// partial-word-on-both-sides path, not just the whole-word fast path
/// `bitmap_find_free_run_respects_alignment` already covers.
#[test_case]
fn bitmap_find_free_run_straddles_word_boundary() {
    let mut words = [0u64; 2]; // 128 frames; word 0 = [0,64), word 1 = [64,128)
    let mut bmp = FrameBitmap::new(&mut words, 0, 128);
    bmp.fill_used();
    for frame in 60..70 {
        bmp.set_free(frame);
    }

    assert_eq!(bmp.find_free_run(8, 1), Some(60));
    assert_eq!(bmp.find_free_run(8, 4), Some(60)); // 60 is itself 4-aligned
}

// --- mm::pmm (brief M1-T2 step 4) ------------------------------------

#[test_case]
fn pmm_stats_sane() {
    let stats = pmm::stats();
    assert!(stats.total > 0, "pmm should have found some RAM");
    assert!(stats.free <= stats.total);
    assert_eq!(stats.used + stats.free, stats.total);
}

#[test_case]
fn pmm_alloc_frame_valid() {
    let phys = pmm::alloc_frame().expect("pmm should have free frames");
    assert!(phys.is_aligned(FRAME_SIZE as u64), "0x{:x} isn't 4 KiB aligned", phys.as_u64());

    let (bitmap_base, bitmap_bytes) = pmm::bitmap_region();
    let addr = phys.as_u64();
    assert!(
        addr < bitmap_base.as_u64() || addr >= bitmap_base.as_u64() + bitmap_bytes as u64,
        "allocated the bitmap's own frame at 0x{addr:x}"
    );

    let mut in_usable_region = false;
    pmm::for_each_usable_region(|base, len| {
        if addr >= base.as_u64() && addr + FRAME_SIZE as u64 <= base.as_u64() + len {
            in_usable_region = true;
        }
    });
    assert!(in_usable_region, "0x{addr:x} is not inside any USABLE region");

    pmm::free_frame(phys);
}

#[test_case]
fn pmm_two_allocations_differ() {
    let a = pmm::alloc_frame().expect("first alloc should succeed");
    let b = pmm::alloc_frame().expect("second alloc should succeed");
    assert_ne!(a, b);
    pmm::free_frame(a);
    pmm::free_frame(b);
}

#[test_case]
fn pmm_alloc_then_free_all_restores_count() {
    let before = pmm::stats().free;

    let mut frames = [PhysAddr::new(0); 64];
    for slot in frames.iter_mut() {
        *slot = pmm::alloc_frame().expect("should have 64 free frames available");
    }
    for &frame in &frames {
        pmm::free_frame(frame);
    }

    assert_eq!(pmm::stats().free, before);
}

/// Dirties a frame, frees it, then immediately calls
/// `alloc_frame_zeroed`: nothing else runs on this single core between
/// the free and the next alloc, and the allocator is a deterministic
/// lowest-free-frame search, so it is guaranteed to hand back the
/// exact same frame -- letting this test prove `alloc_frame_zeroed`
/// actually zeroes it, rather than just trusting RAM happened to
/// start zeroed.
#[test_case]
fn pmm_alloc_frame_zeroed_is_zero() {
    let dirty = pmm::alloc_frame().expect("should have a free frame");
    let dirty_virt = hhdm::phys_to_virt(dirty);
    // SAFETY: `dirty` was just allocated by `alloc_frame`, so this
    // test exclusively owns it for as long as it stays allocated; the
    // HHDM maps every physical frame the PMM covers 1:1, so
    // `dirty_virt` is valid and writable for `FRAME_SIZE` bytes.
    unsafe { core::ptr::write_bytes(dirty_virt.as_u64() as *mut u8, 0xAA, FRAME_SIZE) };
    pmm::free_frame(dirty);

    let zeroed = pmm::alloc_frame_zeroed().expect("should have a free frame");
    assert_eq!(zeroed, dirty, "allocator wasn't deterministic between the free and this alloc");

    let zeroed_virt = hhdm::phys_to_virt(zeroed);
    // SAFETY: `zeroed` was just allocated (and zeroed) by
    // `alloc_frame_zeroed` above; same reasoning as the write above,
    // this time reading the bytes back.
    let bytes = unsafe { core::slice::from_raw_parts(zeroed_virt.as_u64() as *const u8, FRAME_SIZE) };
    assert!(bytes.iter().all(|&b| b == 0), "alloc_frame_zeroed did not zero the frame");

    pmm::free_frame(zeroed);
}

#[test_case]
fn pmm_alloc_contiguous_aligned() {
    let phys = pmm::alloc_contiguous(8, 8).expect("should find a free 8-frame run");
    assert_eq!(
        phys.as_u64() % (8 * FRAME_SIZE as u64),
        0,
        "0x{:x} isn't aligned to 32 KiB",
        phys.as_u64()
    );

    let base_frame = phys.frame_index();
    for i in 0..8u64 {
        let frame_addr = PhysAddr::new((base_frame + i) * FRAME_SIZE as u64);
        assert!(
            pmm::is_frame_used(frame_addr),
            "frame {} of the contiguous run isn't marked used",
            base_frame + i
        );
    }

    pmm::free_contiguous(phys, 8);
}

#[test_case]
fn pmm_frame_zero_never_returned() {
    for _ in 0..1000 {
        let phys = pmm::alloc_frame().expect("should have free frames for 1000 cycles");
        assert_ne!(phys.as_u64(), 0, "frame 0 was handed out by alloc_frame");
        pmm::free_frame(phys);
    }
}

/// `stats().low_reserved` (kernel-review, M1-T2 fix #2) should reflect
/// the frames below 1 MiB that Limine called USABLE -- QEMU's default
/// map always has some (conventional-memory `0x0-0x9fc00`-ish), and it
/// can never exceed the 256 frames a 1 MiB / 4 KiB split allows for.
#[test_case]
fn pmm_stats_low_reserved_sane() {
    let stats = pmm::stats();
    assert!(stats.low_reserved > 0, "low memory reservation should never be zero on QEMU");
    assert!(stats.low_reserved <= 256, "can't reserve more than the 256 frames below 1 MiB");
}

#[test_case]
fn pmm_alloc_frame_low_returns_low_address() {
    let phys = pmm::alloc_frame_low().expect("should have a free low-memory frame");
    assert!(phys.as_u64() < 0x10_0000, "0x{:x} is not below 1 MiB", phys.as_u64());
    assert!(phys.is_aligned(FRAME_SIZE as u64));
    assert_ne!(phys.as_u64(), 0, "frame 0 was handed out by alloc_frame_low");

    pmm::free_frame(phys);
}
