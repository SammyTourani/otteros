//! 4-level page table entries and tables (brief M1-T4). DECISIONS.md D2:
//! page-table entries are ours to define, not the (forbidden) `x86_64`
//! crate's. `mm::vmm` is the only intended caller of the table-walking
//! helpers here; everything a page table describes is always reached
//! through the HHDM (`mm::hhdm`), never through a recursive mapping, so
//! these helpers work identically whether the table in question is
//! currently loaded into CR3 or not.

use super::addr::{FRAME_SIZE, PhysAddr, VirtAddr};
use super::{hhdm, pmm};

/// How many entries a page table has at every level (Intel SDM Vol. 3A
/// 4.5): one 4 KiB table holds 512 eight-byte entries.
pub const ENTRIES_PER_TABLE: usize = 512;

/// Page-table entry flag bits (Intel SDM Vol. 3A Table 4-19). `HUGE` (the
/// `PS` bit) only means anything at the PDPT/PD level; `NO_EXECUTE` (bit
/// 63) only takes effect once `mm::vmm::init_kernel_space` sets
/// `EFER.NXE` (`arch::x86_64::cr::EFER_NXE_BIT`) -- before that it's
/// simply reserved-must-be-zero, which every entry already satisfies.
///
/// A hand-rolled bitset rather than the `bitflags` crate (DECISIONS.md D2
/// allows it, but doesn't require it): ten fixed bits with no need for
/// iteration/parsing/debug-formatting beyond what's written here don't
/// carry their weight as a dependency.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct PageFlags(u64);

impl PageFlags {
    pub const NONE: Self = Self(0);
    pub const PRESENT: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const USER: Self = Self(1 << 2);
    pub const WRITE_THROUGH: Self = Self(1 << 3);
    pub const NO_CACHE: Self = Self(1 << 4);
    pub const ACCESSED: Self = Self(1 << 5);
    pub const DIRTY: Self = Self(1 << 6);
    pub const HUGE: Self = Self(1 << 7);
    pub const GLOBAL: Self = Self(1 << 8);
    pub const NO_EXECUTE: Self = Self(1 << 63);

    /// Every bit this type ever sets; masking with this is how
    /// `PageTableEntry::flags` strips the physical-address bits back out.
    const MASK: u64 = Self::PRESENT.0
        | Self::WRITABLE.0
        | Self::USER.0
        | Self::WRITE_THROUGH.0
        | Self::NO_CACHE.0
        | Self::ACCESSED.0
        | Self::DIRTY.0
        | Self::HUGE.0
        | Self::GLOBAL.0
        | Self::NO_EXECUTE.0;

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn from_bits_truncate(bits: u64) -> Self {
        Self(bits & Self::MASK)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl core::ops::BitOr for PageFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for PageFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::fmt::Debug for PageFlags {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PageFlags(0x{:x})", self.0)
    }
}

/// Bits 12..51: where every entry (huge or not) stores its physical
/// address. A 2 MiB (huge) entry's low 21 bits are simply always zero by
/// alignment, so the same mask reads back a 2 MiB-aligned base correctly
/// too.
const PHYS_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Alignment a `HUGE` entry's address must satisfy (Intel SDM Vol. 3A
/// Table 4-18: bits 20..12 of a 2 MiB PD entry are reserved-must-be-zero).
const HUGE_PAGE_ALIGN: u64 = 2 * 1024 * 1024;

/// One page-table entry at any of the four levels (Intel SDM Vol. 3A
/// Table 4-19/4-20). Whether it's a leaf (maps memory) or points at
/// another table depends on the level it lives at and, for PDPT/PD
/// entries, its own `HUGE` bit -- `PageTableEntry` itself doesn't know or
/// care which; that's `mm::vmm::AddressSpace`'s job.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub fn is_present(self) -> bool {
        self.0 & PageFlags::PRESENT.bits() != 0
    }

    pub fn is_huge(self) -> bool {
        self.0 & PageFlags::HUGE.bits() != 0
    }

    /// The physical address this entry stores, regardless of whether it's
    /// present, huge, or neither -- callers that care check `is_present`/
    /// `is_huge` themselves first.
    pub fn addr(self) -> PhysAddr {
        PhysAddr::new(self.0 & PHYS_ADDR_MASK)
    }

    pub fn flags(self) -> PageFlags {
        PageFlags::from_bits_truncate(self.0)
    }

    /// Overwrites this entry to point at `addr` with `flags`.
    ///
    /// # Panics
    /// If `addr` isn't 4 KiB aligned (every physical address this type
    /// ever stores -- frame, page table, or a 2 MiB-aligned huge page --
    /// is at least that aligned), or if `flags` has `HUGE` set but `addr`
    /// isn't 2 MiB aligned (kernel-review fix #6: a huge entry's address
    /// bits below bit 21 are architecturally reserved-must-be-zero --
    /// Intel SDM Vol. 3A Table 4-18 -- so a merely-4-KiB-aligned huge
    /// mapping would silently truncate to the wrong 2 MiB frame instead
    /// of failing loudly here).
    pub fn set(&mut self, addr: PhysAddr, flags: PageFlags) {
        assert!(addr.is_aligned(FRAME_SIZE as u64), "PageTableEntry::set: {addr:?} isn't 4 KiB aligned");
        if flags.contains(PageFlags::HUGE) {
            assert!(addr.is_aligned(HUGE_PAGE_ALIGN), "PageTableEntry::set: {addr:?} (HUGE) isn't 2 MiB aligned");
        }
        self.0 = (addr.as_u64() & PHYS_ADDR_MASK) | (flags.bits() & !PHYS_ADDR_MASK);
    }

    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

/// A single 4 KiB, 512-entry page table (PML4, PDPT, PD or PT -- the
/// layout is identical at every level; Intel SDM Vol. 3A 4.5).
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; ENTRIES_PER_TABLE],
}

impl PageTable {
    pub const fn empty() -> Self {
        Self { entries: [PageTableEntry::empty(); ENTRIES_PER_TABLE] }
    }

    pub fn entry(&self, index: usize) -> PageTableEntry {
        self.entries[index]
    }

    pub fn entry_mut(&mut self, index: usize) -> &mut PageTableEntry {
        &mut self.entries[index]
    }
}

/// Extracts each level's 9-bit index out of a virtual address (Intel SDM
/// Vol. 3A Figure 4-8): bits 47..39 (PML4), 38..30 (PDPT), 29..21 (PD),
/// 20..12 (PT).
pub const fn pml4_index(virt: VirtAddr) -> usize {
    ((virt.as_u64() >> 39) & 0x1ff) as usize
}

pub const fn pdpt_index(virt: VirtAddr) -> usize {
    ((virt.as_u64() >> 30) & 0x1ff) as usize
}

pub const fn pd_index(virt: VirtAddr) -> usize {
    ((virt.as_u64() >> 21) & 0x1ff) as usize
}

pub const fn pt_index(virt: VirtAddr) -> usize {
    ((virt.as_u64() >> 12) & 0x1ff) as usize
}

/// Follows a present, non-huge `entry` to the next-level table it points
/// at, through the HHDM. `None` if `entry` isn't present.
///
/// # Safety
/// `entry` must not have `HUGE` set (a huge entry's "address" is a data
/// frame, not a page table -- reinterpreting it as one is exactly the
/// unsafety this contract rules out), and if present, must come from a
/// table this module built (so its address is a real, exclusively-owned,
/// zeroed-then-`PageTableEntry`-populated 4 KiB frame, reachable at
/// `mm::hhdm::phys_to_virt(entry.addr())`).
pub unsafe fn next_table(entry: PageTableEntry) -> Option<&'static mut PageTable> {
    if !entry.is_present() {
        return None;
    }
    debug_assert!(!entry.is_huge(), "paging::next_table: entry is a huge (leaf) mapping, not a table");
    let virt = hhdm::phys_to_virt(entry.addr());
    // SAFETY: forwarded from this function's own contract.
    Some(unsafe { &mut *(virt.as_u64() as *mut PageTable) })
}

/// Like `next_table`, but allocates and zeroes a fresh frame for `entry`
/// first if it isn't present yet (a zeroed frame is exactly an empty
/// `PageTable`: every `PageTableEntry` is `0`, i.e. not present).
///
/// `parent_flags` should always be permissive (`PRESENT | WRITABLE`, and
/// `USER` for a table that will ever hold user-accessible leaves): Intel
/// SDM Vol. 3A 4.6 ANDs the writable/user/executable bits across every
/// level used to translate an address, so *only* the leaf entry should
/// ever restrict anything -- a restriction set here would silently
/// override a more permissive leaf placed under it later.
///
/// # Panics
/// If allocating a frame for a new table fails (out of memory this early
/// in boot is unrecoverable).
///
/// # Safety
/// Same contract as `next_table`.
// TODO(kernel-review M1-T4 fix #4, post-M1): the frame this allocates for
// a new intermediate table is never freed or refcounted -- once a
// PML4/PDPT/PD slot gets a table here, that frame lives for the lifetime
// of the address space (or forever, for the kernel's own), even if every
// leaf entry under it is later `unmap`'d. Fine for M1 (a handful of
// long-lived address spaces, no process teardown yet); needs a refcount
// per intermediate table (freed back to the PMM when it hits zero) once
// M2 tears down user address spaces.
pub unsafe fn next_table_or_create(entry: &mut PageTableEntry, parent_flags: PageFlags) -> &'static mut PageTable {
    if !entry.is_present() {
        let frame = pmm::alloc_frame_zeroed().expect("paging::next_table_or_create: out of memory for a page table");
        entry.set(frame, parent_flags | PageFlags::PRESENT);
    }
    // SAFETY: forwarded from this function's own contract; `entry` is now
    // present (already was, or was just made so above) and, by that same
    // contract, not huge.
    unsafe { next_table(*entry).expect("just ensured present") }
}
