//! Higher-half direct map (HHDM) bookkeeping (brief M1-T2 step 2;
//! DECISIONS.md D3, D15). Limine identity-maps the whole of physical
//! memory starting at a fixed virtual offset it chooses and reports at
//! boot; the kernel's own page tables (M1-T4) reproduce that exact
//! mapping, so recording the offset once here -- rather than touching CR3
//! -- lets every `mm` module (and everything after it) treat "physical ->
//! virtual" as a cheap addition for the rest of the kernel's life.
//!
//! This module has no idea Limine exists: `mm::init` reads the HHDM
//! response and passes just the offset to `init` below, so the request/
//! response plumbing lives in one place (`mm::mod`) and this stays pure
//! arithmetic over an address the caller already resolved.

use core::sync::atomic::{AtomicU64, Ordering};

use super::addr::{PhysAddr, VirtAddr};

/// Sentinel for "not yet initialised". Never confused with a real offset:
/// Limine's HHDM offset is always a canonical, page-aligned higher-half
/// address, and `u64::MAX` is neither.
const UNSET: u64 = u64::MAX;

static OFFSET: AtomicU64 = AtomicU64::new(UNSET);

/// Records the HHDM offset Limine reported. Must run before
/// `phys_to_virt`/`virt_to_phys_hhdm` are ever called -- `mm::init` does
/// this first, before handing the memory map to `pmm::init`, which needs
/// both.
pub fn init(offset: u64) {
    OFFSET.store(offset, Ordering::Release);
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
/// another HHDM-relative pointer, e.g. one Limine handed us directly) --
/// debug-asserts `virt` is actually at or above the HHDM offset, since
/// passing an arbitrary kernel/user virtual address here would silently
/// produce a nonsense physical address instead of a compile-time error.
pub fn virt_to_phys_hhdm(virt: VirtAddr) -> PhysAddr {
    let off = offset();
    debug_assert!(
        virt.as_u64() >= off,
        "0x{:x} is below the HHDM offset 0x{:x}",
        virt.as_u64(),
        off
    );
    PhysAddr::new(virt.as_u64() - off)
}
