//! ECAM (Enhanced Configuration Access Mechanism) for PCI config space
//! access via memory-mapped I/O. The MCFG table tells us where to map
//! the ECAM window, and ECAM offset = (bus << 20) | (dev << 15) | (fn << 12) | reg.

use crate::mm::addr::PhysAddr;
use crate::mm::hhdm;

/// Maps the ECAM window for PCI config space access (uncached MMIO).
/// The caller must ensure this happens once per MCFG segment and the
/// mapped region lives for the kernel's lifetime.
pub fn map_ecam_window(base_addr: PhysAddr, _size: u64) -> *const u8 {
    // Brief D15: MMIO windows go through the kernel dynamic region or HHDM.
    // We use HHDM if the address is below 4 GiB (which it typically is in QEMU),
    // otherwise we'd need to allocate from the kernel dynamic region.
    // For simplicity, we assume it's HHDM-mapped.
    let virt = hhdm::phys_to_virt(base_addr);
    virt.as_u64() as *const u8
}

/// Reads a u32 from PCI config space at the given offset.
/// offset must be 4-byte aligned.
///
/// # Safety
/// The caller guarantees ecam_base points to a valid ECAM window
/// (mapped via map_ecam_window above), offset is within bounds, and
/// offset is 4-byte aligned (PCI config space requirement).
pub unsafe fn read_u32(ecam_base: *const u8, offset: u32) -> u32 {
    // SAFETY: Guaranteed by caller; volatile read prevents optimization.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe {
        let addr = ecam_base.add(offset as usize) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Writes a u32 to PCI config space at the given offset.
/// offset must be 4-byte aligned.
///
/// # Safety
/// Same as read_u32.
pub unsafe fn write_u32(ecam_base: *const u8, offset: u32, value: u32) {
    // SAFETY: Guaranteed by caller.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe {
        let addr = (ecam_base as *mut u8).add(offset as usize) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

/// Reads a u16 from PCI config space at the given offset.
///
/// # Safety
/// Same as read_u32, but allows any offset (u16 reads don't need
/// alignment beyond their size).
pub unsafe fn read_u16(ecam_base: *const u8, offset: u32) -> u16 {
    // SAFETY: Guaranteed by caller.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe {
        let addr = ecam_base.add(offset as usize) as *const u16;
        core::ptr::read_volatile(addr)
    }
}

/// Writes a u16 to PCI config space at the given offset.
///
/// # Safety
/// Same as read_u16.
pub unsafe fn write_u16(ecam_base: *const u8, offset: u32, value: u16) {
    // SAFETY: Guaranteed by caller.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe {
        let addr = (ecam_base as *mut u8).add(offset as usize) as *mut u16;
        core::ptr::write_volatile(addr, value);
    }
}

/// Reads a u8 from PCI config space at the given offset.
///
/// # Safety
/// Byte reads have no alignment requirement, but the caller must
/// guarantee ecam_base is valid and offset is in bounds.
pub unsafe fn read_u8(ecam_base: *const u8, offset: u32) -> u8 {
    // SAFETY: Guaranteed by caller.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe {
        let addr = ecam_base.add(offset as usize);
        core::ptr::read_volatile(addr)
    }
}

/// Writes a u8 to PCI config space at the given offset.
///
/// # Safety
/// Same as read_u8.
pub unsafe fn write_u8(ecam_base: *const u8, offset: u32, value: u8) {
    // SAFETY: Guaranteed by caller.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe {
        let addr = (ecam_base as *mut u8).add(offset as usize);
        core::ptr::write_volatile(addr, value);
    }
}
