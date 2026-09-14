//! Kernel heap tests (brief M1-T3): `alloc::{vec, string, boxed,
//! collections}` actually working, backed by the PMM, with no leaks under
//! stress. Every test that measures `heap::stats()`/`pmm::stats()` before
//! and after fully drains whatever it allocated first -- see
//! `heap_interleaved_alloc_free_stable`'s doc comment for why that
//! guarantees the counts return to their exact starting values regardless
//! of what slabs earlier tests happened to leave behind.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::alloc::Layout;

use otteros_kernel::mm::{heap, pmm};

/// A `Box<u64>` round-trips through the heap: write, read back, drop.
#[test_case]
fn heap_box_roundtrip() {
    let b = Box::new(0x1234_5678_9abc_def0u64);
    assert_eq!(*b, 0x1234_5678_9abc_def0u64);
    let value = *b;
    drop(b);
    assert_eq!(value, 0x1234_5678_9abc_def0u64);
}

/// `Vec<u64>` growing to 10,000 elements keeps every value, including
/// across whatever reallocations `push` triggers along the way.
#[test_case]
fn heap_vec_push_10000_sum() {
    let mut v: Vec<u64> = Vec::new();
    for i in 0..10_000u64 {
        v.push(i);
    }
    assert_eq!(v.len(), 10_000);
    let sum: u64 = v.iter().sum();
    assert_eq!(sum, 10_000 * 9_999 / 2);
}

/// `alloc::format!` builds exactly the string it should -- exercises
/// `String`'s own growth on top of the heap, not just a fixed-size type.
#[test_case]
fn heap_format_builds_string() {
    let s = alloc::format!("otteros heap {} {:04}", "ready", 7);
    assert_eq!(s, "otteros heap ready 0007");
}

/// `BTreeMap<u32, u32>` with 1,000 inserts, then a lookup for every key.
#[test_case]
fn heap_btreemap_1000_inserts() {
    let mut map: BTreeMap<u32, u32> = BTreeMap::new();
    for k in 0..1000u32 {
        map.insert(k, k.wrapping_mul(31).wrapping_add(7));
    }
    assert_eq!(map.len(), 1000);
    for k in 0..1000u32 {
        assert_eq!(map.get(&k), Some(&k.wrapping_mul(31).wrapping_add(7)), "lookup failed for key {k}");
    }
}

/// `Layout::from_size_align(64, 64)`: a small, over-aligned request served
/// by the slab path (class 64, itself naturally 64-byte aligned).
#[test_case]
fn heap_layout_alignment_64() {
    let layout = Layout::from_size_align(64, 64).expect("64/64 is a valid layout");
    // SAFETY: `layout` has non-zero size; `ptr` is deallocated with this
    // exact layout below before the test returns, and nothing else can
    // reach it in between (thread-less kernel, single test at a time).
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    assert!(!ptr.is_null(), "a 64-byte allocation should not fail this early in boot");
    assert_eq!(ptr as usize % 64, 0, "pointer {ptr:p} isn't 64-byte aligned");
    // SAFETY: `ptr` is valid for `layout.size()` (64) bytes, just allocated.
    unsafe { core::ptr::write_bytes(ptr, 0xAB, 64) };
    // SAFETY: `ptr` was allocated with exactly `layout` above, and this is
    // the only, final use of it.
    unsafe { alloc::alloc::dealloc(ptr, layout) };
}

/// `Layout::from_size_align(4096, 4096)`: exactly at the boundary the brief
/// calls out (alignment *above* 4096 is unsupported, but 4096 itself must
/// work) -- routed to the large-object path, one whole frame.
#[test_case]
fn heap_layout_alignment_4096() {
    let layout = Layout::from_size_align(4096, 4096).expect("4096/4096 is a valid layout");
    let pmm_before = pmm::stats().free;

    // SAFETY: see `heap_layout_alignment_64`.
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    assert!(!ptr.is_null(), "a single free frame should be available");
    assert_eq!(ptr as usize % 4096, 0, "pointer {ptr:p} isn't 4096-byte aligned");
    // SAFETY: `ptr` is valid for 4096 bytes, just allocated above.
    unsafe { core::ptr::write_bytes(ptr, 0xCD, 4096) };
    // SAFETY: matches the layout used to allocate; this is its only,
    // final use.
    unsafe { alloc::alloc::dealloc(ptr, layout) };

    assert_eq!(pmm::stats().free, pmm_before, "the frame backing it should be returned to the PMM");
}

/// A 1 MiB allocation (comfortably into the large-object path: 256
/// frames) is writable end to end, and dropping it returns every one of
/// those frames to the PMM.
#[test_case]
fn heap_large_allocation_roundtrip() {
    let pmm_before = pmm::stats().free;
    {
        let mut buf: Vec<u8> = alloc::vec![0u8; 1024 * 1024];
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = (i % 256) as u8;
        }
        for (i, &byte) in buf.iter().enumerate() {
            assert_eq!(byte, (i % 256) as u8, "byte {i} of the 1 MiB buffer was corrupted");
        }
    }
    assert_eq!(pmm::stats().free, pmm_before, "PMM free-frame count should be restored after drop");
}

/// 10,000 small allocations interleaved with frees through a small rotating
/// window of live objects (each new allocation drops whatever previously
/// occupied its slot): heap `bytes_in_use` and PMM free frames must return
/// to exactly their starting values once every slot is drained at the end.
///
/// This holds regardless of what other tests left behind in the same slab
/// class: every alloc this test performs is matched by exactly one dealloc
/// before the final assertions (nothing here is left live), and each
/// class's free-object count only ever moves by +-1 per alloc/dealloc --
/// so however much slab capacity this test borrows from (or lends back to)
/// slabs other tests already created, the net effect by the time it's done
/// is zero, both for `bytes_in_use` and for whichever frames the PMM had
/// to hand out along the way.
#[test_case]
fn heap_interleaved_alloc_free_stable() {
    const WINDOW: usize = 16;

    let heap_before = heap::stats().bytes_in_use;
    let pmm_before = pmm::stats().free;

    let mut live: [Option<Box<u64>>; WINDOW] = core::array::from_fn(|_| None);
    for i in 0..10_000u64 {
        let slot = (i as usize) % WINDOW;
        live[slot] = Some(Box::new(i)); // drops whatever was in `slot` first
    }
    for slot in live.iter_mut() {
        *slot = None; // drain the rest
    }

    assert_eq!(heap::stats().bytes_in_use, heap_before, "heap bytes_in_use should return to baseline");
    assert_eq!(pmm::stats().free, pmm_before, "PMM free frame count should return to baseline");
}

/// A `Vec<u8>` grown one byte at a time past 2048 elements crosses from the
/// slab path onto the large-object path at least once; every byte must
/// still be exactly what was pushed, proving growth actually copies (or
/// otherwise preserves) old contents rather than losing them at a class
/// boundary.
#[test_case]
fn heap_vec_grows_across_size_classes() {
    let mut v: Vec<u8> = Vec::new();
    for i in 0..5000u32 {
        v.push((i % 256) as u8);
    }
    assert_eq!(v.len(), 5000);
    assert!(v.capacity() > 2048, "5000 bytes should have crossed onto the large-object path");
    for (i, &byte) in v.iter().enumerate() {
        assert_eq!(byte, (i % 256) as u8, "byte {i} corrupted somewhere during growth");
    }
}

/// The byte this test's stress pattern expects at position `i` of the
/// buffer allocated as sequence number `seq` (kernel-review, M1-T3 fix
/// #4): position-dependent, not just a per-buffer constant, so two
/// aliasing buffers can't coincide (a constant-fill pattern can't tell
/// "byte 3 of buffer A" apart from "byte 3 of buffer B" if they overlap;
/// this can, since they almost certainly disagree at any given offset).
fn stress_pattern_byte(seq: u32, i: usize) -> u8 {
    ((seq.wrapping_mul(2_654_435_761) >> 24) as u8) ^ (i as u8)
}

/// A deterministic LCG drives 5,000 alloc/free operations over a small
/// working set of live buffers, sizes 1..=8192 (spanning every slab class
/// and the large-object path), each filled with `stress_pattern_byte` and
/// verified just before the buffer is reused or freed -- any overlap
/// between two live allocations, or any corruption of one by another,
/// flips at least one byte and fails the assertion.
#[test_case]
fn heap_lcg_stress_5000_ops() {
    const LIVE: usize = 32;

    let heap_before = heap::stats().bytes_in_use;
    let pmm_before = pmm::stats().free;

    // Each live slot remembers the buffer and the sequence number (unique
    // per allocation, never reused) its pattern was derived from.
    let mut slots: [Option<(Vec<u8>, u32)>; LIVE] = core::array::from_fn(|_| None);
    // Knuth's MMIX LCG constants; the seed is fixed so this test is exactly
    // reproducible.
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;

    for seq in 0..5000u32 {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let idx = (state >> 33) as usize % LIVE;
        let size = 1 + (state >> 16) as usize % 8192; // 1..=8192

        if let Some((old_buf, old_seq)) = slots[idx].take() {
            assert!(
                old_buf.iter().enumerate().all(|(i, &b)| b == stress_pattern_byte(old_seq, i)),
                "corruption detected reusing live slot {idx}"
            );
        }

        let mut buf: Vec<u8> = alloc::vec![0u8; size];
        for (i, byte) in buf.iter_mut().enumerate() {
            *byte = stress_pattern_byte(seq, i);
        }
        slots[idx] = Some((buf, seq));
    }

    for (idx, slot) in slots.iter_mut().enumerate() {
        if let Some((buf, seq)) = slot.take() {
            assert!(
                buf.iter().enumerate().all(|(i, &b)| b == stress_pattern_byte(seq, i)),
                "corruption detected in final live slot {idx}"
            );
        }
    }

    assert_eq!(heap::stats().bytes_in_use, heap_before, "heap bytes_in_use should return to baseline");
    assert_eq!(pmm::stats().free, pmm_before, "PMM free frame count should return to baseline");
}

/// The large-object table holds at most 512 concurrent allocations
/// (kernel-review, M1-T3 fix #5): the 513th must fail closed (`alloc`
/// returns null) rather than panic or silently overflow the table.
/// Draining all 512 afterwards must return every frame to the PMM.
#[test_case]
fn heap_large_table_capacity_returns_null_at_513() {
    let layout = Layout::from_size_align(4096, 8).expect("4096/8 is a valid layout");
    let pmm_before = pmm::stats().free;

    let mut ptrs: [*mut u8; 512] = [core::ptr::null_mut(); 512];
    for slot in ptrs.iter_mut() {
        // SAFETY: `layout` has non-zero size; every successful pointer is
        // deallocated with this exact layout below before the test ends.
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        assert!(!ptr.is_null(), "the large-object table should have room for 512 concurrent allocations");
        *slot = ptr;
    }

    // SAFETY: `layout` has non-zero size; a null return means nothing was
    // allocated, so there is nothing to free for this one.
    let overflow = unsafe { alloc::alloc::alloc(layout) };
    assert!(overflow.is_null(), "the 513th concurrent large allocation should fail, not succeed");

    for &ptr in &ptrs {
        // SAFETY: `ptr` was returned by a successful `alloc` above with
        // this exact `layout`, and is freed here exactly once.
        unsafe { alloc::alloc::dealloc(ptr, layout) };
    }

    assert_eq!(pmm::stats().free, pmm_before, "PMM free frame count should return to baseline");
}

/// `Layout::from_size_align(64, 8192)`: alignment above 4096 is
/// unsupported (brief M1-T3) regardless of how small `size` is -- `alloc`
/// must return null without panicking, and touch neither the heap nor the
/// PMM stats.
#[test_case]
fn heap_align_above_4096_returns_null() {
    let layout = Layout::from_size_align(64, 8192).expect("64/8192 is a valid layout");
    let heap_before = heap::stats().bytes_in_use;
    let pmm_before = pmm::stats().free;

    // SAFETY: `layout` has non-zero size; a null return means nothing was
    // allocated, so there is nothing to free afterwards.
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    assert!(ptr.is_null(), "alignment above 4096 should fail, not succeed");

    assert_eq!(heap::stats().bytes_in_use, heap_before, "a rejected allocation must not touch heap stats");
    assert_eq!(pmm::stats().free, pmm_before, "a rejected allocation must not touch PMM stats");
}
