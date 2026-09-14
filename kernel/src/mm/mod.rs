//! Physical memory management (brief M1-T2): address newtypes (`addr`),
//! higher-half direct map bookkeeping (`hhdm`), a frame-granularity bitmap
//! (`bitmap`), and the physical memory manager built on top of it (`pmm`).
//! DECISIONS.md D2 (write the allocator ourselves -- `spin` is allowed for
//! the lock), D3 (Limine HHDM + memory map), D5 (serial logging).

pub mod addr;
pub mod bitmap;
pub mod hhdm;
pub mod pmm;

use limine::request::{HhdmRequest, MemmapRequest};

use crate::{kprintln, qemu};

#[used]
#[unsafe(link_section = ".requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".requests")]
static MEMMAP_REQUEST: MemmapRequest = MemmapRequest::new();

/// Brings up physical memory management: records the HHDM offset Limine
/// reported, then hands the Limine memory map to `pmm::init`. Must run
/// after `serial::init()` (both log) and before anything allocates a frame
/// (the heap, M1-T3, onward).
pub fn init() {
    let hhdm_offset = match HHDM_REQUEST.response() {
        Some(response) => response.offset,
        None => {
            kprintln!("[pmm] FATAL: no HHDM response from Limine");
            qemu::exit(false);
        }
    };

    let entries = match MEMMAP_REQUEST.response() {
        Some(response) => response.entries(),
        None => {
            kprintln!("[pmm] FATAL: no memory map response from Limine");
            qemu::exit(false);
        }
    };

    // The highest physical address any memory-map entry mentions (not just
    // USABLE ones): the HHDM is only known-good up to here, so `hhdm`
    // needs it up front to bounds-check `virt_to_phys_hhdm` for real
    // (kernel-review, M1-T2 fix #5).
    let highest_physical_end = entries.iter().map(|e| e.base + e.length).max().unwrap_or(0);
    hhdm::init(hhdm_offset, highest_physical_end);

    pmm::init(entries);
}
