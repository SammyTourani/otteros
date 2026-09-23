//! The virtual memory manager (brief M1-T4): builds the kernel's own
//! 4-level page tables from the Limine memory map and switches CR3 to
//! them, then exposes `AddressSpace::{map_4k,map_2m,map_range,unmap,
//! translate,activate}` for everything that follows (MMIO in a later
//! task, user processes and shared memory in M2 -- `teardown` is a
//! process's side of that, brief M2-T2). DECISIONS.md D15 is the
//! address layout this module builds; D2 forbids the `x86_64` crate for
//! the CR0/CR3/CR4/EFER/`invlpg` access `arch::x86_64::cr` provides
//! instead.
//!
//! Two invariants everything here leans on:
//! - Every page table this module ever builds is reached through the HHDM
//!   (`mm::hhdm::phys_to_virt`), never a recursive mapping -- so building
//!   and walking tables works identically whether they're the ones
//!   currently loaded into CR3 or not (in particular, while still running
//!   under Limine's own tables, before `activate()`).
//! - `init_kernel_space` reproduces the exact HHDM Limine already set up
//!   (same offset, same physical coverage) and maps the kernel image at
//!   its existing higher-half address, so every pointer the kernel
//!   already holds -- HHDM aliases, the framebuffer, Limine responses,
//!   the heap's slabs/large objects, the PMM bitmap -- stays valid across
//!   the switch.

mod teardown;

use core::sync::atomic::{AtomicU64, Ordering};

use limine::memmap::{self, Entry};
use limine::request::ExecutableAddressRequest;

use super::addr::{FRAME_SIZE, PhysAddr, VirtAddr};
use super::paging::{self, PageFlags, PageTable};
use super::{hhdm, pmm};
use crate::arch::x86_64::cr;
use crate::{kprintln, qemu};

#[used]
#[unsafe(link_section = ".requests")]
static EXECUTABLE_ADDRESS_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();

const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;
/// Sentinel for "`init_kernel_space` hasn't run yet". `0` is never a valid
/// PML4 physical address (frame 0 is never allocated -- see
/// `pmm::free_frame`'s docs).
const UNSET: u64 = 0;

static KERNEL_PML4: AtomicU64 = AtomicU64::new(UNSET);

/// A page's size, as returned by `AddressSpace::translate`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageSize {
    /// A normal, 4 KiB (`FRAME_SIZE`) page.
    Normal,
    /// A 2 MiB huge page.
    Huge,
}

/// One virtual address space: a PML4 and the map/unmap/translate/activate
/// operations that maintain and load it. Cheap to copy (it's just the
/// physical address of the root table); every method reaches the actual
/// tables through the HHDM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressSpace {
    pml4: PhysAddr,
}

/// Permissions granted to every intermediate (PML4/PDPT/PD-as-table)
/// entry this module creates for a leaf mapped with `leaf_flags`.
/// Deliberately maximal otherwise (present, writable, never `NO_EXECUTE`):
/// see `paging::next_table_or_create`'s docs for why only leaf entries
/// should ever restrict anything.
///
/// `USER`, uniquely among the flags this module hands callers, is *not*
/// leaf-only (brief M2-T2, kernel-review-worthy fix): Intel SDM Vol. 3A
/// 4.6 ANDs the writable/user/executable bits across *every* level a
/// translation walks through, so a leaf marked `USER` sitting under an
/// otherwise-maximal-but-`USER`-less PDPT/PD/PT (which is exactly what
/// this function used to return unconditionally) is still supervisor-only
/// in practice -- ring 3 takes a `#PF` on the very first access, no matter
/// what the leaf itself says. Propagating `leaf_flags`'s own `USER` bit up
/// to every intermediate level a *new* table is created for fixes this;
/// it's sound because this kernel never shares an intermediate table
/// between a kernel-half (PML4 256..511) and a user-half (0..256) mapping
/// (D15), so nothing with `USER` unset ever needs an intermediate table
/// that some *other*, `USER`-set mapping also passes through.
fn intermediate_flags(leaf_flags: PageFlags) -> PageFlags {
    let mut flags = PageFlags::PRESENT | PageFlags::WRITABLE;
    if leaf_flags.contains(PageFlags::USER) {
        flags |= PageFlags::USER;
    }
    flags
}

impl AddressSpace {
    /// The physical address of this address space's PML4 (for `CR3`,
    /// logging, and tests).
    pub fn pml4(&self) -> PhysAddr {
        self.pml4
    }

    /// Test-only introspection: PML4 entry `index`'s raw bits (address +
    /// flags), so a test can compare two address spaces' entries without
    /// needing `PageTable`/`PageTableEntry` to be part of the public API.
    ///
    /// # Panics
    /// If `index >= paging::ENTRIES_PER_TABLE` (512).
    pub fn pml4_entry_raw(&self, index: usize) -> u64 {
        self.pml4_table().entry(index).bits()
    }

    /// `pub(super)`, not private (brief M2-T2): `teardown`, a sibling
    /// submodule, walks the same tables this does and needs the identical
    /// HHDM-backed access.
    pub(super) fn pml4_table(&self) -> &'static mut PageTable {
        let virt = hhdm::phys_to_virt(self.pml4);
        // SAFETY: `self.pml4` was allocated by `pmm::alloc_frame_zeroed`
        // (in `init_kernel_space`/`new_user`) and is never freed while
        // this `AddressSpace` (or any copy of it) is in use, so `virt` is
        // a valid, exclusively-`PageTable`-shaped 4 KiB frame reachable
        // through the HHDM. Single-threaded, interrupts off: nothing else
        // observes this table concurrently, and every caller here finishes
        // walking down from one level before touching the next, so the
        // `&'static mut` this hands out is never aliased by another live
        // reference to the same frame.
        unsafe { &mut *(virt.as_u64() as *mut PageTable) }
    }

    /// Maps one 4 KiB page. `flags` is ORed with `PRESENT` automatically
    /// -- callers only need to spell out the permission bits that vary
    /// (`WRITABLE`, `USER`, `NO_EXECUTE`, `GLOBAL`, ...).
    ///
    /// # Panics
    /// If `virt`/`phys` aren't 4 KiB aligned, if `virt` already falls
    /// inside an existing 2 MiB mapping, or if `virt` is already mapped
    /// (a kernel bug -- remapping a page is never legitimate; unmap it
    /// first).
    pub fn map_4k(&self, virt: VirtAddr, phys: PhysAddr, flags: PageFlags) {
        assert!(virt.is_aligned(FRAME_SIZE as u64), "vmm::map_4k: {virt:?} isn't 4 KiB aligned");
        assert!(phys.is_aligned(FRAME_SIZE as u64), "vmm::map_4k: {phys:?} isn't 4 KiB aligned");

        let pml4 = self.pml4_table();
        // SAFETY: `pml4`'s entries are either not-yet-present or were
        // themselves created by `next_table_or_create` (so, not huge --
        // only `map_2m` ever sets `HUGE`, one level further down).
        let pdpt = unsafe { paging::next_table_or_create(pml4.entry_mut(paging::pml4_index(virt)), intermediate_flags(flags)) };
        // SAFETY: same reasoning, one level down.
        let pd = unsafe { paging::next_table_or_create(pdpt.entry_mut(paging::pdpt_index(virt)), intermediate_flags(flags)) };

        let pd_entry = pd.entry_mut(paging::pd_index(virt));
        assert!(
            !pd_entry.is_huge(),
            "vmm::map_4k: {virt:?} falls inside an existing 2 MiB mapping at {:?}",
            pd_entry.addr()
        );
        // SAFETY: `pd_entry` was just confirmed not huge, and is either
        // not-yet-present or points at a table `next_table_or_create`
        // built earlier.
        let pt = unsafe { paging::next_table_or_create(pd_entry, intermediate_flags(flags)) };

        let entry = pt.entry_mut(paging::pt_index(virt));
        assert!(
            !entry.is_present(),
            "vmm::map_4k: {virt:?} is already mapped to {:?} (cannot remap to {phys:?})",
            entry.addr()
        );
        entry.set(phys, flags | PageFlags::PRESENT);
        // SAFETY: `virt` is a page-aligned kernel virtual address; there
        // was no valid translation for it before this call (just
        // confirmed above), so invalidating any stale/absent TLB entry
        // for it is always sound.
        unsafe { cr::invlpg(virt.as_u64()) };
    }

    /// Maps one 2 MiB huge page. `flags` is ORed with `PRESENT | HUGE`
    /// automatically; see `map_4k`.
    ///
    /// # Panics
    /// If `virt`/`phys` aren't 2 MiB aligned, or if `virt` is already
    /// mapped.
    pub fn map_2m(&self, virt: VirtAddr, phys: PhysAddr, flags: PageFlags) {
        assert!(virt.is_aligned(HUGE_PAGE_SIZE), "vmm::map_2m: {virt:?} isn't 2 MiB aligned");
        assert!(phys.is_aligned(HUGE_PAGE_SIZE), "vmm::map_2m: {phys:?} isn't 2 MiB aligned");

        let pml4 = self.pml4_table();
        // SAFETY: see `map_4k`.
        let pdpt = unsafe { paging::next_table_or_create(pml4.entry_mut(paging::pml4_index(virt)), intermediate_flags(flags)) };
        // SAFETY: see `map_4k`.
        let pd = unsafe { paging::next_table_or_create(pdpt.entry_mut(paging::pdpt_index(virt)), intermediate_flags(flags)) };

        let entry = pd.entry_mut(paging::pd_index(virt));
        assert!(
            !entry.is_present(),
            "vmm::map_2m: {virt:?} is already mapped to {:?} (cannot remap to {phys:?})",
            entry.addr()
        );
        entry.set(phys, flags | PageFlags::PRESENT | PageFlags::HUGE);
        // SAFETY: see `map_4k`.
        unsafe { cr::invlpg(virt.as_u64()) };
    }

    /// Maps `[virt, virt + len)` to `[phys, phys + len)`, using 2 MiB
    /// pages for any aligned-on-both-sides, long-enough-remaining run and
    /// 4 KiB pages for the rest (the usual case for a range that doesn't
    /// start/end on a 2 MiB boundary: a 4 KiB-page "collar" on one or both
    /// ends around a huge-paged middle).
    ///
    /// # Panics
    /// If `virt`/`phys` aren't 4 KiB aligned, if `len` is zero or not a
    /// multiple of `FRAME_SIZE` (kernel-review fix #5: a non-page-multiple
    /// `len` would otherwise silently map one page short, or a caller
    /// meaning "nothing to map" should say so explicitly rather than pay
    /// for a zero-iteration loop that looks like a bug at the call site),
    /// or (via `map_4k`/`map_2m`) if any page in the range is already
    /// mapped.
    pub fn map_range(&self, virt: VirtAddr, phys: PhysAddr, len: u64, flags: PageFlags) {
        assert!(virt.is_aligned(FRAME_SIZE as u64), "vmm::map_range: {virt:?} isn't 4 KiB aligned");
        assert!(phys.is_aligned(FRAME_SIZE as u64), "vmm::map_range: {phys:?} isn't 4 KiB aligned");
        assert!(
            len != 0 && len.is_multiple_of(FRAME_SIZE as u64),
            "vmm::map_range: len 0x{len:x} is zero or not a multiple of FRAME_SIZE"
        );

        let mut done = 0u64;
        while done < len {
            let v = virt.as_u64() + done;
            let p = phys.as_u64() + done;
            let remaining = len - done;

            if v.is_multiple_of(HUGE_PAGE_SIZE) && p.is_multiple_of(HUGE_PAGE_SIZE) && remaining >= HUGE_PAGE_SIZE {
                self.map_2m(VirtAddr::new(v), PhysAddr::new(p), flags);
                done += HUGE_PAGE_SIZE;
            } else {
                self.map_4k(VirtAddr::new(v), PhysAddr::new(p), flags);
                done += FRAME_SIZE as u64;
            }
        }
    }

    /// Removes the mapping for `virt`'s page (4 KiB or, if `virt` falls
    /// inside one, 2 MiB), returning the physical address it mapped to,
    /// or `None` if it wasn't mapped. Frees nothing -- the caller (still)
    /// owns whatever frame(s) that was.
    // TODO(kernel-review M1-T4 fix #4, post-M1): this only ever clears the
    // one leaf entry -- the PDPT/PD/PT frames `next_table`/
    // `next_table_or_create` walked through to reach it are never
    // inspected, freed, or refcounted, even if this was the very last
    // present entry in one of them (a now-fully-empty page table is
    // simply left allocated). See `paging::next_table_or_create`'s
    // identical TODO; same fix, same later task.
    pub fn unmap(&self, virt: VirtAddr) -> Option<PhysAddr> {
        assert!(virt.is_aligned(FRAME_SIZE as u64), "vmm::unmap: {virt:?} isn't 4 KiB aligned");

        let pml4 = self.pml4_table();
        // SAFETY: see `map_4k`.
        let pdpt = unsafe { paging::next_table(pml4.entry(paging::pml4_index(virt))) }?;
        // SAFETY: see `map_4k`.
        let pd = unsafe { paging::next_table(pdpt.entry(paging::pdpt_index(virt))) }?;

        let pd_entry = pd.entry_mut(paging::pd_index(virt));
        if pd_entry.is_huge() {
            if !pd_entry.is_present() {
                return None;
            }
            let phys = pd_entry.addr();
            pd_entry.clear();
            // SAFETY: `virt` is a valid kernel virtual address that was
            // just mapped (2 MiB huge) the instant before this clear.
            unsafe { cr::invlpg(virt.as_u64()) };
            return Some(phys);
        }

        // SAFETY: `pd_entry` was just confirmed not huge; see `map_4k`.
        let pt = unsafe { paging::next_table(*pd_entry) }?;
        let pt_entry = pt.entry_mut(paging::pt_index(virt));
        if !pt_entry.is_present() {
            return None;
        }
        let phys = pt_entry.addr();
        pt_entry.clear();
        // SAFETY: `virt` is a valid kernel virtual address that was just
        // mapped the instant before this clear.
        unsafe { cr::invlpg(virt.as_u64()) };
        Some(phys)
    }

    /// Resolves `virt` to `(physical address, leaf flags, page size)`, or
    /// `None` if it isn't mapped at all.
    pub fn translate(&self, virt: VirtAddr) -> Option<(PhysAddr, PageFlags, PageSize)> {
        let pml4 = self.pml4_table();
        // SAFETY: see `map_4k`.
        let pdpt = unsafe { paging::next_table(pml4.entry(paging::pml4_index(virt))) }?;
        // SAFETY: see `map_4k`.
        let pd = unsafe { paging::next_table(pdpt.entry(paging::pdpt_index(virt))) }?;

        let pd_entry = pd.entry(paging::pd_index(virt));
        if pd_entry.is_huge() {
            if !pd_entry.is_present() {
                return None;
            }
            let page_offset = virt.as_u64() & (HUGE_PAGE_SIZE - 1);
            let phys = PhysAddr::new(pd_entry.addr().as_u64() + page_offset);
            return Some((phys, pd_entry.flags(), PageSize::Huge));
        }

        // SAFETY: `pd_entry` was just confirmed not huge; see `map_4k`.
        let pt = unsafe { paging::next_table(pd_entry) }?;
        let pt_entry = pt.entry(paging::pt_index(virt));
        if !pt_entry.is_present() {
            return None;
        }
        let page_offset = virt.as_u64() & (FRAME_SIZE as u64 - 1);
        let phys = PhysAddr::new(pt_entry.addr().as_u64() + page_offset);
        Some((phys, pt_entry.flags(), PageSize::Normal))
    }

    /// Loads this address space's PML4 into CR3.
    pub fn activate(&self) {
        // SAFETY: `self.pml4` is a fully-built PML4 -- either the kernel
        // one `init_kernel_space` just finished (reproducing the HHDM and
        // the kernel image mapping, so every pointer already in use stays
        // valid), or a `new_user` one whose upper half is that exact same
        // kernel space -- so every mapping the CPU relies on immediately
        // before and after this call means the same thing.
        unsafe { cr::write_cr3(self.pml4) };
    }

    /// Builds a fresh, otherwise-empty address space for a future user
    /// process (M2): a new PML4 with PML4 entries 256..511 copied
    /// verbatim from the kernel's (DECISIONS.md D15: "PML4 entries
    /// 256..511 are shared by every address space" -- copying the raw
    /// entries makes every user address space share the exact same
    /// underlying kernel PDPT/PD/PT frames, not just equal-looking ones).
    /// Entries 0..256 (user space) start unmapped.
    pub fn new_user() -> Self {
        let new_pml4_phys = pmm::alloc_frame_zeroed().expect("vmm::new_user: out of memory for a PML4");
        let space = Self { pml4: new_pml4_phys };

        let kernel = kernel_address_space();
        let kernel_table = kernel.pml4_table();
        let new_table = space.pml4_table();
        for index in 256..paging::ENTRIES_PER_TABLE {
            *new_table.entry_mut(index) = kernel_table.entry(index);
        }

        space
    }
}

/// The current kernel address space (valid once `init_kernel_space` has
/// run).
///
/// # Panics
/// If `init_kernel_space` hasn't run yet.
pub fn kernel_address_space() -> AddressSpace {
    let raw = KERNEL_PML4.load(Ordering::Acquire);
    assert_ne!(raw, UNSET, "vmm::kernel_address_space: init_kernel_space was never called");
    AddressSpace { pml4: PhysAddr::new(raw) }
}

/// The kernel image's link-time virtual base and Limine-assigned physical
/// base: `(virtual_base, physical_base)`, such that for any address `v`
/// inside the kernel image, its physical address is `physical_base + (v -
/// virtual_base)`. Exposed for tests (brief M1-T4); `map_kernel_image`
/// uses the identical relationship internally.
pub fn kernel_image_bounds() -> (VirtAddr, PhysAddr) {
    let exe = EXECUTABLE_ADDRESS_REQUEST
        .response()
        .expect("vmm::kernel_image_bounds: Limine never answered the executable-address request");
    (VirtAddr::new(exe.virtual_base), PhysAddr::new(exe.physical_base))
}

/// Builds the kernel's own page tables -- the kernel image (W^X, from the
/// executable-address response and the linker's section symbols) and the
/// HHDM (every memory-map entry except `BAD_MEMORY`, plus any gap below
/// 4 GiB no entry mentions at all, so unlisted MMIO holes like the LAPIC
/// (0xFEE00000) and I/O APIC (0xFEC00000) stay reachable) -- then enables
/// `EFER.NXE`/`CR0.WP`/`CR4.PGE` and switches CR3 to them (brief M1-T4
/// step 3). Called once, from `init()`, while still running on the stack
/// Limine handed the kernel; `mm::kstack::init_boot_stack` moves off that
/// stack immediately afterward.
pub fn init_kernel_space() -> AddressSpace {
    let pml4_phys = pmm::alloc_frame_zeroed().expect("vmm::init_kernel_space: out of memory for the kernel PML4");
    let space = AddressSpace { pml4: pml4_phys };

    map_kernel_image(&space);
    map_hhdm(&space);

    // SAFETY: enabling `EFER.NXE`, `CR0.WP` and `CR4.PGE` only ever makes
    // an existing translation *more* restrictive (NX/WP) or extends how
    // long a global TLB entry survives a CR3 reload (PGE); every mapping
    // built just above already assumes all three (NX on rodata/data, W^X
    // on the kernel image, GLOBAL everywhere), and nothing between here
    // and `activate()` below writes to a read-only page or executes from
    // a non-executable one, so raising these bits *before* the switch --
    // while still running on Limine's own tables -- can't fault.
    unsafe {
        let efer = cr::rdmsr(cr::EFER_MSR);
        cr::wrmsr(cr::EFER_MSR, efer | cr::EFER_NXE_BIT);
        cr::write_cr0(cr::read_cr0() | cr::CR0_WP);
        cr::write_cr4(cr::read_cr4() | cr::CR4_PGE);
    }

    space.activate();
    KERNEL_PML4.store(pml4_phys.as_u64(), Ordering::Release);
    kprintln!("[vmm] switched to kernel page tables (pml4=0x{:x})", pml4_phys.as_u64());

    space
}

// The linker-defined section-boundary symbols (kernel/linker-x86_64.ld,
// brief M1-T4): zero-sized marker symbols, so only their *addresses* are
// ever meaningful, never a "value" read through them.
unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
}

/// Maps the kernel image W^X: text RX, rodata R (+NX), data/bss RW (+NX),
/// all GLOBAL. `map_4k`/`map_2m` never add `PRESENT` on their own (or any
/// other flag) -- every leaf flag, including `PRESENT`, is spelled out at
/// each call site below so the intent (RX/R/RW) reads directly off it.
fn map_kernel_image(space: &AddressSpace) {
    let exe = EXECUTABLE_ADDRESS_REQUEST.response().unwrap_or_else(|| {
        kprintln!("[vmm] FATAL: no executable-address response from Limine");
        qemu::exit(false);
    });
    let phys_base = exe.physical_base;
    let virt_base = exe.virtual_base;
    let to_phys = |virt: u64| PhysAddr::new(phys_base + (virt - virt_base));

    // `&raw const` only ever *forms* a raw pointer -- it never reads
    // through the (extern, so ordinarily reference-unsafe) static -- so
    // none of this needs an `unsafe` block; see gdt.rs's `Tss::set_rsp0`
    // docs for the same reasoning applied to unaligned fields instead of
    // extern statics.
    let (text_start, text_end, rodata_start, rodata_end, data_start, data_end) = (
        &raw const __text_start as u64,
        &raw const __text_end as u64,
        &raw const __rodata_start as u64,
        &raw const __rodata_end as u64,
        &raw const __data_start as u64,
        &raw const __data_end as u64,
    );

    space.map_range(VirtAddr::new(text_start), to_phys(text_start), text_end - text_start, PageFlags::GLOBAL);
    space.map_range(
        VirtAddr::new(rodata_start),
        to_phys(rodata_start),
        rodata_end - rodata_start,
        PageFlags::GLOBAL | PageFlags::NO_EXECUTE,
    );
    space.map_range(
        VirtAddr::new(data_start),
        to_phys(data_start),
        data_end - data_start,
        PageFlags::WRITABLE | PageFlags::GLOBAL | PageFlags::NO_EXECUTE,
    );
}

/// Upper bound on how many merged physical ranges `map_hhdm` tracks at
/// once -- generous headroom over what real firmware/QEMU ever report
/// (see `pmm::MAX_USABLE_REGIONS`, which is more tightly scoped to just
/// USABLE entries and still uses 64).
const MAX_MEMMAP_RANGES: usize = 128;

/// Reproduces Limine's HHDM: every memory-map entry except `BAD_MEMORY`
/// (brief M1-T4 step 3), plus any gap below 4 GiB that no entry mentions
/// at all -- real memory maps have plenty of these (e.g. the legacy
/// 0xA0000-0x100000 hole) and firmware/hardware are free to leave fixed
/// MMIO windows like the LAPIC (0xFEE00000) or I/O APIC (0xFEC00000)
/// inside one without ever mentioning them, so without this,
/// `hhdm::phys_to_virt` would compute an address for them that this
/// kernel's own page tables simply never map.
///
/// Entries are rounded outward to whole pages and merged (`merge_ranges`)
/// before any of this is mapped: real memory maps -- particularly the
/// legacy BIOS/e820 one this project's `bios-test` exercises -- are not
/// required to have 4 KiB-aligned entries (e.g. the classic EBDA boundary
/// at 0x9fc00, confirmed the hard way: `map_range` panicking on exactly
/// that address under `--firmware bios`). Merging first is what makes
/// this safe either way: `map_range` never sees a misaligned start, and
/// two entries whose *rounded* bounds now overlap or touch are mapped
/// once as a single range instead of two separate (and therefore
/// spuriously conflicting) calls.
fn map_hhdm(space: &AddressSpace) {
    // `super::memmap_entries()`, not a second `MemmapRequest`: see that
    // function's docs for why a duplicate request hangs Limine outright.
    let entries = super::memmap_entries();
    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE | PageFlags::GLOBAL;

    let mut mappable = [(0u64, 0u64); MAX_MEMMAP_RANGES];
    let mappable_count = merge_ranges(entries, |e| e.type_ != memmap::MEMMAP_BAD_MEMORY, u64::MAX, &mut mappable);

    // Kernel-review fix #2: a BAD_MEMORY entry's *rounded* bounds can
    // share a page with a neighbouring good entry's *own* rounded bounds
    // (both round outward independently -- see `merge_ranges`), so
    // filtering BAD_MEMORY out before rounding/merging (as above) isn't
    // enough on its own: that shared page would still get mapped, via the
    // neighbour. Rounding and merging BAD_MEMORY the same way and then
    // subtracting it from `mappable` is what actually guarantees a page
    // touched by bad memory is never mapped, no matter which entry it's
    // attributed to.
    let mut bad = [(0u64, 0u64); MAX_MEMMAP_RANGES];
    let bad_count = merge_ranges(entries, |e| e.type_ == memmap::MEMMAP_BAD_MEMORY, u64::MAX, &mut bad);

    let mut to_map = [(0u64, 0u64); MAX_MEMMAP_RANGES];
    let to_map_count = subtract_ranges(&mappable[..mappable_count], &bad[..bad_count], &mut to_map);
    for &(start, end) in &to_map[..to_map_count] {
        map_hhdm_gap(space, start, end, flags);
    }

    // Fill every stretch of [0, 4 GiB) that no entry (of *any* type,
    // including BAD_MEMORY -- explicitly-described-but-unmapped still
    // isn't a "gap") describes at all.
    const CEILING: u64 = 4 * 1024 * 1024 * 1024;
    let mut known = [(0u64, 0u64); MAX_MEMMAP_RANGES];
    let known_count = merge_ranges(entries, |_| true, CEILING, &mut known);
    let mut next_free = 0u64;
    for &(start, end) in &known[..known_count] {
        map_hhdm_gap(space, next_free, start, flags);
        next_free = next_free.max(end);
    }
    map_hhdm_gap(space, next_free, CEILING, flags);
}

/// Maps `[start, end)` (physical) through the HHDM, unless the range is
/// empty.
fn map_hhdm_gap(space: &AddressSpace, start: u64, end: u64, flags: PageFlags) {
    if end <= start {
        return;
    }
    let virt = hhdm::phys_to_virt(PhysAddr::new(start));
    space.map_range(virt, PhysAddr::new(start), end - start, flags);
}

/// Builds a sorted, merged, non-overlapping, page-aligned (rounded
/// outward) set of `[start, end)` ranges from every entry `keep` accepts,
/// clipped to `ceiling`, into `out`. Returns how many ranges it wrote.
///
/// # Panics
/// If there are more entries (before merging) than `out.len()` -- see
/// `MAX_MEMMAP_RANGES`.
fn merge_ranges(entries: &[&Entry], keep: impl Fn(&Entry) -> bool, ceiling: u64, out: &mut [(u64, u64)]) -> usize {
    let mut count = 0usize;
    for entry in entries.iter().filter(|e| keep(e)) {
        let start = entry.base.min(ceiling);
        let end = (entry.base + entry.length).min(ceiling);
        if end <= start {
            continue;
        }
        let start = PhysAddr::new(start).align_down(FRAME_SIZE as u64).as_u64();
        let end = PhysAddr::new(end).align_up(FRAME_SIZE as u64).as_u64();

        assert!(count < out.len(), "vmm::merge_ranges: more memory-map entries than this table can hold");
        out[count] = (start, end);
        count += 1;
    }

    let slice = &mut out[..count];
    slice.sort_unstable_by_key(|&(start, _)| start);

    // In-place merge: `write` (always `<= read`, so `slice[read]` is never
    // clobbered before it's read) tracks how many finalized, non-
    // overlapping ranges precede the range currently being considered.
    let mut write = 0usize;
    for read in 0..count {
        if write > 0 && slice[read].0 <= slice[write - 1].1 {
            slice[write - 1].1 = slice[write - 1].1.max(slice[read].1);
        } else {
            slice[write] = slice[read];
            write += 1;
        }
    }
    write
}

/// Subtracts every range in `subtract` (sorted, non-overlapping -- e.g.
/// `merge_ranges`'s output) from every range in `from` (same), writing
/// the result -- still sorted, non-overlapping -- into `out`. Returns how
/// many ranges it wrote. Splits a `from` range in two if a `subtract`
/// range falls entirely inside it.
///
/// # Panics
/// If the result has more ranges than `out.len()` -- each `subtract`
/// range can split at most one `from` range into two, so
/// `from.len() + subtract.len()` (both bounded by `MAX_MEMMAP_RANGES`) is
/// always enough headroom.
fn subtract_ranges(from: &[(u64, u64)], subtract: &[(u64, u64)], out: &mut [(u64, u64)]) -> usize {
    let mut count = 0usize;
    for &(from_start, from_end) in from {
        let mut cursor = from_start;
        for &(bad_start, bad_end) in subtract {
            if bad_end <= cursor || bad_start >= from_end {
                continue; // this bad range doesn't touch what's left of `from` at all.
            }
            if bad_start > cursor {
                assert!(count < out.len(), "vmm::subtract_ranges: more resulting ranges than this table can hold");
                out[count] = (cursor, bad_start);
                count += 1;
            }
            cursor = cursor.max(bad_end);
            if cursor >= from_end {
                break;
            }
        }
        if cursor < from_end {
            assert!(count < out.len(), "vmm::subtract_ranges: more resulting ranges than this table can hold");
            out[count] = (cursor, from_end);
            count += 1;
        }
    }
    count
}
