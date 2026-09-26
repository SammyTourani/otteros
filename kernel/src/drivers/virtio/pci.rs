//! Shared virtio PCI transport code (brief M5-T1a): capability walking and MMIO transport.

use crate::mm::addr::VirtAddr;

/// MMIO transport for virtio device communication.
pub struct VirtioTransport {
    common_config_virt: VirtAddr,
    notify_base_virt: VirtAddr,
    device_config_virt: VirtAddr,
    notify_off_multiplier: u32,
}

impl otter_virtio::Transport for VirtioTransport {
    fn device_feature(&self, select: u32) -> u32 {
        // SAFETY: common_config_virt points to a valid, mapped MMIO region mapped
        // by mmio::map() with uncached flags. Offsets 0x00 and 0x04 correspond to
        // device_feature_select and device_feature registers (virtio 1.2 §4.1.4.3).
        unsafe {
            let ptr = self.common_config_virt.as_u64() as *mut u32;
            core::ptr::write_volatile(ptr, select);
            let ptr = (self.common_config_virt.as_u64() + 0x04) as *const u32;
            core::ptr::read_volatile(ptr)
        }
    }

    fn set_driver_feature(&self, select: u32, value: u32) {
        // SAFETY: common_config_virt is a valid MMIO region. Offsets 0x08 and 0x0C
        // correspond to driver_feature_select and driver_feature registers.
        unsafe {
            core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x08) as *mut u32, select);
            core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x0C) as *mut u32, value);
        }
    }

    fn status(&self) -> u8 {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x14 is the device_status register.
        unsafe { core::ptr::read_volatile((self.common_config_virt.as_u64() + 0x14) as *const u8) }
    }

    fn set_status(&self, s: u8) {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x14 is the device_status register.
        unsafe { core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x14) as *mut u8, s); }
    }

    fn num_queues(&self) -> u16 {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x12 is the num_queues register.
        unsafe { core::ptr::read_volatile((self.common_config_virt.as_u64() + 0x12) as *const u16) }
    }

    fn select_queue(&self, index: u16) {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x16 is the queue_select register.
        unsafe { core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x16) as *mut u16, index); }
    }

    fn queue_size(&self) -> u16 {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x18 is the queue_size register.
        unsafe { core::ptr::read_volatile((self.common_config_virt.as_u64() + 0x18) as *const u16) }
    }

    fn set_queue_size(&self, size: u16) {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x18 is the queue_size register.
        unsafe { core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x18) as *mut u16, size); }
    }

    fn set_queue_msix_vector(&self, v: u16) {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x1A is the queue_msix_vector register.
        unsafe { core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x1A) as *mut u16, v); }
    }

    fn queue_msix_vector(&self) -> u16 {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x1A is the queue_msix_vector register.
        unsafe { core::ptr::read_volatile((self.common_config_virt.as_u64() + 0x1A) as *const u16) }
    }

    fn set_queue_addresses(&self, desc: u64, driver: u64, device: u64) {
        // SAFETY: common_config_virt is a valid MMIO region. Offsets 0x20, 0x28, 0x30
        // correspond to queue_desc, queue_driver, and queue_device registers.
        unsafe {
            core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x20) as *mut u64, desc);
            core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x28) as *mut u64, driver);
            core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x30) as *mut u64, device);
        }
    }

    fn set_queue_enable(&self, enable: bool) {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x1C is the queue_enable register.
        unsafe {
            core::ptr::write_volatile((self.common_config_virt.as_u64() + 0x1C) as *mut u16, if enable { 1 } else { 0 });
        }
    }

    fn queue_notify_off(&self) -> u16 {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x1E is the queue_notify_off register.
        unsafe { core::ptr::read_volatile((self.common_config_virt.as_u64() + 0x1E) as *const u16) }
    }

    fn config_generation(&self) -> u32 {
        // SAFETY: common_config_virt is a valid MMIO region. Offset 0x15 is the config_generation register.
        unsafe { core::ptr::read_volatile((self.common_config_virt.as_u64() + 0x15) as *const u8) as u32 }
    }

    fn read_config(&self, offset: usize, buf: &mut [u8]) {
        // SAFETY: device_config_virt points to a valid MMIO region for device-specific configuration.
        let base = self.device_config_virt.as_u64() + offset as u64;
        for (i, byte) in buf.iter_mut().enumerate() {
            unsafe { *byte = core::ptr::read_volatile((base + i as u64) as *const u8); }
        }
    }

    fn notify(&self, _queue: u16, notify_off: u16) {
        // SAFETY: notify_base_virt points to a valid MMIO notify region mapped by mmio::map().
        // The notify offset is calculated using notify_off_multiplier per the virtio spec.
        unsafe {
            let offset = notify_off as u64 * self.notify_off_multiplier as u64;
            core::ptr::write_volatile((self.notify_base_virt.as_u64() + offset) as *mut u16, 0);
        }
    }
}

impl VirtioTransport {
    /// Create a new VirtioTransport from the given MMIO addresses.
    pub fn new(
        common_config_virt: VirtAddr,
        notify_base_virt: VirtAddr,
        device_config_virt: VirtAddr,
        notify_off_multiplier: u32,
    ) -> Self {
        Self {
            common_config_virt,
            notify_base_virt,
            device_config_virt,
            notify_off_multiplier,
        }
    }
}

/// Helper function to read a u8 from PCI config space.
/// # Safety
/// Caller must ensure `base` is a valid ECAM window mapped by map_ecam_window, and the
/// computed offset is within the PCI config space (0x0 - 0xff).
pub unsafe fn read_pci_u8(base: *const u8, ecam_offset: u32, cap_offset: u8, offset: usize) -> u8 {
    // SAFETY: Caller ensures base is valid and offset is within bounds.
    unsafe { crate::drivers::pci::ecam::read_u8(base, ecam_offset + (cap_offset as u32) + offset as u32) }
}

/// Helper function to read a u32 from PCI config space.
/// # Safety
/// Caller must ensure `base` is a valid ECAM window mapped by map_ecam_window, and the
/// computed offset is within the PCI config space (0x0 - 0xff).
pub unsafe fn read_pci_u32(base: *const u8, ecam_offset: u32, cap_offset: u8, offset: usize) -> u32 {
    // SAFETY: Caller ensures base is valid and offset is within bounds.
    unsafe { crate::drivers::pci::ecam::read_u32(base, ecam_offset + (cap_offset as u32) + offset as u32) }
}

/// Helper function to write a u16 to PCI config space.
/// # Safety
/// Caller must ensure `base` is a valid ECAM window mapped by map_ecam_window, and the
/// computed offset is within the PCI config space.
pub unsafe fn write_pci_u16(base: *const u8, ecam_offset: u32, offset: u32, value: u16) {
    // SAFETY: Caller ensures base is valid and offset is within bounds.
    unsafe { crate::drivers::pci::ecam::write_u16(base, ecam_offset + offset, value) }
}
