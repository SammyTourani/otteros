//! `mm::paging`/`mm::vmm`/`mm::kstack` tests (brief M1-T4): the kernel's
//! own page tables, already active (`otteros_kernel::init` switches CR3
//! and the kernel stack before `test_main` ever runs -- see lib.rs), so
//! every test here exercises the *real*, currently-loaded address space,
//! not a scratch one.

use otteros_kernel::arch::x86_64::cr;
use otteros_kernel::mm::addr::{FRAME_SIZE, PhysAddr, VirtAddr};
use otteros_kernel::mm::paging::PageFlags;
use otteros_kernel::mm::vmm::{self, AddressSpace, PageSize};
use otteros_kernel::mm::{hhdm, kstack, pmm};
use otteros_kernel::sched;

/// An ordinary immutable static: never mutated through a Rust reference,
/// so (like `gdt.rs`'s `KERNEL_STACK`/`TSS`/`GDT` statics document for the
/// opposite case) the compiler is free to -- and does -- place it in
/// `.rodata`, exactly what `vmm_rodata_pages_are_nx` needs a real address
/// inside `.rodata` for.
static RODATA_PROBE: u32 = 0xABCD_1234;

/// A `static mut`: written through a raw pointer elsewhere in a real
/// kernel, but never here -- only its *address* is taken (`&raw const`,
/// which never needs `unsafe`; see gdt.rs's `Tss::set_rsp0` docs for the
/// same reasoning), enough to land it in `.data`/`.bss` for
/// `vmm_data_pages_are_writable_and_nx`.
static mut DATA_PROBE: u32 = 0xDEAD_BEEF;

/// Two distinct, fixed addresses in the kernel dynamic region
/// (DECISIONS.md D15), far past anything `mm::kstack`'s bump allocator
/// will have handed out by the time tests run (one boot stack, well under
/// 1 MiB in): safe for these tests to `map_4k`/`map_2m` directly without
/// colliding with a real kernel stack.
const SCRATCH_4K_VIRT: u64 = 0xffff_c000_1000_0000;
const SCRATCH_2M_VIRT: u64 = 0xffff_c000_2000_0000;

/// `translate(text symbol)` == the executable's physical base plus the
/// symbol's offset from its virtual base -- i.e. `vmm::init_kernel_space`
/// really did map the kernel image at Limine's reported location, not
/// just *some* location.
#[test_case]
fn vmm_translate_text_symbol_matches_kernel_image() {
    let addr = otteros_kernel::hlt_loop as *const () as usize as u64;
    let (virt_base, phys_base) = vmm::kernel_image_bounds();
    let expected = PhysAddr::new(phys_base.as_u64() + (addr - virt_base.as_u64()));

    let (phys, _flags, _size) =
        vmm::kernel_address_space().translate(VirtAddr::new(addr)).expect("hlt_loop should be mapped");
    assert_eq!(phys, expected);
}

/// `translate(hhdm + 0x1000)` == `0x1000`: the HHDM reproduction is a
/// plain identity-plus-offset mapping for ordinary low physical memory,
/// same as Limine's own.
#[test_case]
fn vmm_translate_hhdm_alias_of_low_physical_page() {
    let virt = hhdm::phys_to_virt(PhysAddr::new(0x1000));
    let (phys, _flags, _size) =
        vmm::kernel_address_space().translate(virt).expect("the HHDM alias of 0x1000 should be mapped");
    assert_eq!(phys, PhysAddr::new(0x1000));
}

/// Text pages are executable (not `NO_EXECUTE`) and not `WRITABLE` --
/// W^X, half 1.
#[test_case]
fn vmm_text_pages_are_rx_not_writable() {
    let addr = otteros_kernel::hlt_loop as *const () as usize as u64;
    let (_phys, flags, _size) =
        vmm::kernel_address_space().translate(VirtAddr::new(addr)).expect("hlt_loop should be mapped");
    assert!(!flags.contains(PageFlags::WRITABLE), "text page is writable");
    assert!(!flags.contains(PageFlags::NO_EXECUTE), "text page is NX");
}

/// Rodata pages are `NO_EXECUTE` and not `WRITABLE`.
#[test_case]
fn vmm_rodata_pages_are_nx() {
    let addr = &raw const RODATA_PROBE as u64;
    let (_phys, flags, _size) =
        vmm::kernel_address_space().translate(VirtAddr::new(addr)).expect("RODATA_PROBE should be mapped");
    assert!(flags.contains(PageFlags::NO_EXECUTE), "rodata page isn't NX");
    assert!(!flags.contains(PageFlags::WRITABLE), "rodata page is writable");
}

/// Data/bss pages are `WRITABLE` and `NO_EXECUTE` -- W^X, half 2.
#[test_case]
fn vmm_data_pages_are_writable_and_nx() {
    let addr = &raw const DATA_PROBE as u64;
    let (_phys, flags, _size) =
        vmm::kernel_address_space().translate(VirtAddr::new(addr)).expect("DATA_PROBE should be mapped");
    assert!(flags.contains(PageFlags::WRITABLE), "data page isn't writable");
    assert!(flags.contains(PageFlags::NO_EXECUTE), "data page isn't NX");
}

/// Maps a fresh PMM frame at a dynamic-region address, writes through
/// that mapping, reads the identical bytes back through the frame's HHDM
/// alias (proving both addresses reach the same physical memory), then
/// unmaps it and confirms `translate` forgets it.
#[test_case]
fn vmm_map_write_read_hhdm_then_unmap() {
    let space = vmm::kernel_address_space();
    let phys = pmm::alloc_frame().expect("should have a free frame");
    let virt = VirtAddr::new(SCRATCH_4K_VIRT);

    space.map_4k(virt, phys, PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::GLOBAL);

    // SAFETY: `virt` was just mapped above to `phys`, a frame this test
    // exclusively owns (just allocated, not yet freed); writing exactly
    // `FRAME_SIZE` bytes is in-bounds for a 4 KiB mapping.
    unsafe { core::ptr::write_bytes(virt.as_u64() as *mut u8, 0xAB, FRAME_SIZE) };

    let via_hhdm = hhdm::phys_to_virt(phys);
    // SAFETY: `phys` is the exact frame just written through `virt`; the
    // HHDM maps every PMM frame 1:1, so this alias is valid for
    // `FRAME_SIZE` bytes too.
    let bytes = unsafe { core::slice::from_raw_parts(via_hhdm.as_u64() as *const u8, FRAME_SIZE) };
    assert!(bytes.iter().all(|&b| b == 0xAB), "HHDM alias didn't see the write made through the new mapping");

    assert_eq!(space.unmap(virt), Some(phys));
    assert!(space.translate(virt).is_none(), "translate should forget virt after unmap");

    pmm::free_frame(phys);
}

/// A 2 MiB mapping resolves through `translate` as `PageSize::Huge`, at
/// the right physical address.
#[test_case]
fn vmm_map_2m_translates_as_huge() {
    let space = vmm::kernel_address_space();
    let phys = pmm::alloc_contiguous(512, 512).expect("should find a free 2 MiB-aligned run"); // 512 * 4 KiB = 2 MiB
    let virt = VirtAddr::new(SCRATCH_2M_VIRT);

    space.map_2m(virt, phys, PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::GLOBAL);

    let (t_phys, _flags, size) = space.translate(virt).expect("should be mapped");
    assert_eq!(t_phys, phys);
    assert_eq!(size, PageSize::Huge);

    assert_eq!(space.unmap(virt), Some(phys));
    pmm::free_contiguous(phys, 512);
}

/// CR3 holds exactly the kernel address space's PML4 -- `activate()`
/// really did switch the CPU to it, and it's still active by the time
/// tests run.
#[test_case]
fn vmm_cr3_matches_kernel_pml4() {
    assert_eq!(cr::read_cr3(), vmm::kernel_address_space().pml4().as_u64());
}

/// `AddressSpace::new_user()` copies PML4 entries 256..511 verbatim from
/// the kernel space (DECISIONS.md D15: those entries are shared by every
/// address space) and leaves 0..256 (user space) unmapped.
#[test_case]
fn vmm_new_user_shares_kernel_upper_half() {
    let kernel = vmm::kernel_address_space();
    let user = AddressSpace::new_user();

    for index in 256..512 {
        assert_eq!(
            kernel.pml4_entry_raw(index),
            user.pml4_entry_raw(index),
            "PML4[{index}] should be shared with the kernel space"
        );
    }
    for index in 0..256 {
        assert_eq!(user.pml4_entry_raw(index), 0, "PML4[{index}] (user half) should start unmapped");
    }
}

/// The kernel is, by the time any test runs, already executing on the
/// guard-paged boot stack `mm::kstack::init_boot_stack` allocated: its
/// guard page must not translate (it's deliberately never mapped), while
/// its first real (bottom) page must.
#[test_case]
fn vmm_current_stack_guard_unmapped_bottom_mapped() {
    let stack = kstack::boot_stack();
    let space = vmm::kernel_address_space();
    assert!(space.translate(stack.guard).is_none(), "guard page should not translate");
    assert!(space.translate(stack.bottom).is_some(), "stack bottom should translate");
}

/// `find_guard` recognises the current (boot) stack's own guard page.
#[test_case]
fn vmm_find_guard_locates_boot_stack() {
    let stack = kstack::boot_stack();
    let found = kstack::find_guard(stack.guard).expect("boot stack's guard page should be registered");
    assert_eq!(found.guard.as_u64(), stack.guard.as_u64());
    assert_eq!(found.bottom.as_u64(), stack.bottom.as_u64());
}

/// Kernel-review fix #1 / brief M2-T3: `find_guard` answers purely from
/// arithmetic on the fixed-stride slot layout now (no registry, lock, or
/// shared state of any kind -- the double-fault handler calls it, and
/// must never be able to block) -- if it ever regressed to taking a lock
/// it already held, this plain, same-thread call would deadlock the whole
/// test suite rather than fail quietly. It also doubles as the "rsp-style
/// query" the double-fault handler now uses as its primary signal
/// (kernel-review fix #3): a real overflow's `rsp` lands *inside* the
/// guard page, not necessarily at its first byte, so this queries an
/// address partway in, not `stack.guard` itself (already covered by
/// `vmm_find_guard_locates_boot_stack` above).
#[test_case]
fn vmm_find_guard_matches_rsp_style_query_into_guard_page() {
    let stack = kstack::boot_stack();
    let rsp_like = VirtAddr::new(stack.guard.as_u64() + 512);
    let found = kstack::find_guard(rsp_like).expect("an address inside the guard page should match");
    assert_eq!(found.guard.as_u64(), stack.guard.as_u64());
    assert_eq!(found.bottom.as_u64(), stack.bottom.as_u64());

    // One past the guard page (the stack's first mapped byte) must not
    // match: a half-open range check, not an off-by-one that accepts
    // everything above `guard`.
    assert!(kstack::find_guard(stack.bottom).is_none());
}

/// Brief M2-T3: `mm::kstack` now lays stacks out in fixed-stride slots
/// reused via a free list (replacing a bump allocator with a hard,
/// non-reclaimable 64-lifetime-stack ceiling). After hundreds of
/// allocate/free cycles have reused the same small handful of slots many
/// times over, the purely arithmetic `find_guard` (no runtime registry
/// left to go stale) must still answer correctly for the *freshest*
/// stack: its guard page unmapped and reported by `find_guard`, its first
/// stack page (`bottom`) mapped and never itself reported as a guard.
#[test_case]
fn find_guard_is_correct_after_many_slot_reuses() {
    fn worker(_: usize) -> i32 {
        0
    }
    // Sequential spawn+join, one at a time: each iteration's stack is
    // freed (and its slot recycled) well before the next is allocated, so
    // this drives many reuses of a tiny working set of slots rather than
    // hundreds of distinct, never-before-used ones.
    for _ in 0..300 {
        let id = sched::spawn("test-slot-churn", worker, 0);
        assert_eq!(sched::join(id), 0);
    }

    let id = sched::spawn("test-slot-churn-final", worker, 0);
    let thread = sched::find(id).expect("just spawned");
    let stack = thread.stack();

    let space = vmm::kernel_address_space();
    assert!(space.translate(stack.guard).is_none(), "a (re)used slot's guard page must not translate");
    assert!(space.translate(stack.bottom).is_some(), "a (re)used slot's first stack page must translate");

    let found = kstack::find_guard(stack.guard).expect("the arithmetic guard check should still find a (re)used slot's guard page");
    assert_eq!(found.guard.as_u64(), stack.guard.as_u64());
    assert_eq!(found.bottom.as_u64(), stack.bottom.as_u64());
    assert!(
        kstack::find_guard(stack.bottom).is_none(),
        "a (re)used slot's first stack page must never itself be reported as a guard page"
    );

    assert_eq!(sched::join(id), 0);
}
