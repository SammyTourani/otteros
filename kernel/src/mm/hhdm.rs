//! Higher-half direct map (HHDM) bookkeeping (brief M1-T2 step 2;
//! DECISIONS.md D3, D15). Limine identity-maps the whole of physical
//! memory starting at a fixed virtual offset it chooses and reports at
//! boot; the kernel's own page tables (M1-T4) reproduce that exact
//! mapping, so recording the offset once here -- rather than touching CR3
//! -- lets every `mm` module (and everything after it) treat "physical ->
//! virtual" as a cheap addition for the rest of the kernel's life.
//!
//! This module has no idea Limine exists: `mm::init` reads the HHDM
//! response and the memory map's highest address and passes just those two
//! numbers to `init` below, so the request/response plumbing lives in one
//! place (`mm::mod`) and this stays pure arithmetic over values the caller
//! already resolved.

use core::sync::atomic::{AtomicU64, Ordering};

use super::addr::{PhysAddr, VirtAddr};

/// Sentinel for "not yet initialised". Never confused with a real offset:
/// Limine's HHDM offset is always a canonical, page-aligned higher-half
/// address, and `u64::MAX` is neither.
const UNSET: u64 = u64::MAX;

static OFFSET: AtomicU64 = AtomicU64::new(UNSET);

/// One past the highest physical address the Limine memory map ever
/// mentions (kernel-review, M1-T2 fix #5): the HHDM is only known-good
/// over `0..PHYS_END`, so `virt_to_phys_hhdm` can actually check its
/// input is in range, rather than just "not obviously negative".
static PHYS_END: AtomicU64 = AtomicU64::new(0);

/// Records the HHDM offset Limine reported and the highest physical
/// address `mm::init` saw in the memory map. Must run before
/// `phys_to_virt`/`virt_to_phys_hhdm` are ever called -- `mm::init` does
/// this first, before handing the memory map to `pmm::init`, which needs
/// both.
pub fn init(offset: u64, highest_physical_end: u64) {
    OFFSET.store(offset, Ordering::Release);
    PHYS_END.store(highest_physical_end, Ordering::Release);
}

fn offset() -> u64 {
    let value = OFFSET.load(Ordering::Acquire);
    debug_assert_ne!(value, UNSET, "mm::hhdm::init was never called");
    value
}

/// Translates a physical address to its HHDM virtual alias.
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    VirtAddr::new(phys.as_u64() + offset())
}

/// Translates an HHDM virtual address back to the physical address it
/// aliases. Only meaningful for addresses obtained from `phys_to_virt` (or
/// another HHDM-relative pointer, e.g. one Limine handed us directly).
///
/// # Panics
/// If `virt` doesn't lie in `[offset, offset + highest_physical_end)` --
/// below the HHDM base, or far enough above it that it can't be a valid
/// HHDM alias of memory this kernel knows about. A real `assert!`, kept in
/// release builds: a bogus reverse translation here would silently hand
/// back a nonsense `PhysAddr` that every later `mm`/PMM operation would
/// then trust.
pub fn virt_to_phys_hhdm(virt: VirtAddr) -> PhysAddr {
    let off = offset();
    let end = PHYS_END.load(Ordering::Acquire);
    let addr = virt.as_u64();
    assert!(
        addr >= off && addr < off + end,
        "virt_to_phys_hhdm: 0x{addr:x} is outside the HHDM range [0x{off:x}, 0x{:x})",
        off + end
    );
    PhysAddr::new(addr - off)
}
