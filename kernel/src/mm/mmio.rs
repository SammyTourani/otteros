//! MMIO register mapping (brief M3-T1b): maps device register ranges into
//! a dedicated window of the kernel dynamic region, 0xffff_c800_0000_0000 up
//! to 0xffff_c900_0000_0000, using a page-granular bump allocator.
//! Never use the HHDM for device registers: it is cacheable and may not
//! cover 64-bit BARs (D15).

use super::addr::{PhysAddr, VirtAddr, FRAME_SIZE};
use super::paging::PageFlags;
use super::vmm;
use crate::sync::IrqMutex;

/// Start of the MMIO window in the kernel dynamic region.
const MMIO_WINDOW_START: u64 = 0xffff_c800_0000_0000;
/// End of the MMIO window (exclusive).
const MMIO_WINDOW_END: u64 = 0xffff_c900_0000_0000;

/// The MMIO allocator: tracks the next available virtual address in the window.
static MMIO_ALLOCATOR: IrqMutex<u64> = IrqMutex::new(MMIO_WINDOW_START);

/// Maps a physical address range into the MMIO window and returns its
/// virtual address. Page-granular: the returned virtual address preserves
/// the offset within the page from `phys`. Mapping the same physical page
/// twice may reuse the earlier mapping.
///
/// # Returns
/// The virtual address of `phys` within the mapped region, or `None` if
/// the window is exhausted.
pub fn map(phys: PhysAddr, len: usize) -> Option<VirtAddr> {
    if len == 0 {
        return None;
    }

    // Align the physical address down to the page boundary.
    let phys_aligned = PhysAddr::new(phys.as_u64() & !(FRAME_SIZE as u64 - 1));
    // Preserve the page offset.
    let page_offset = phys.as_u64() & (FRAME_SIZE as u64 - 1);

    // Calculate the number of pages needed to cover the range, starting from
    // the aligned physical address.
    let end_phys = phys.as_u64() + len as u64;
    let phys_aligned_end = (end_phys + FRAME_SIZE as u64 - 1) & !(FRAME_SIZE as u64 - 1);
    let num_pages = ((phys_aligned_end - phys_aligned.as_u64()) / FRAME_SIZE as u64) as usize;

    // Get the current allocation pointer and check bounds.
    let mut allocator = MMIO_ALLOCATOR.lock();
    let virt_base = *allocator;
    let virt_end = virt_base + (num_pages * FRAME_SIZE) as u64;

    if virt_end > MMIO_WINDOW_END {
        return None;
    }

    // Allocate the range and advance the pointer.
    *allocator = virt_end;
    drop(allocator);

    // Map each page through the kernel's address space.
    let kernel_space = vmm::kernel_address_space();
    for i in 0..num_pages {
        let virt = VirtAddr::new(virt_base + (i as u64 * FRAME_SIZE as u64));
        let phys_page = PhysAddr::new(phys_aligned.as_u64() + (i as u64 * FRAME_SIZE as u64));

        // Check if this page is already mapped; if so, skip it.
        if kernel_space.translate(virt).is_some() {
            continue;
        }

        // Map the page with device-appropriate flags.
        kernel_space.map_4k(
            virt,
            phys_page,
            PageFlags::WRITABLE
                | PageFlags::NO_CACHE
                | PageFlags::WRITE_THROUGH
                | PageFlags::NO_EXECUTE
                | PageFlags::GLOBAL,
        );
    }

    // Return the virtual address with the original page offset preserved.
    Some(VirtAddr::new(virt_base + page_offset))
}
