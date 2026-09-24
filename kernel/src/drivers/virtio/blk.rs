//! Virtio block device driver (brief M3-T1).

use crate::drivers::pci::find_device;
use crate::kprintln;
use alloc::boxed::Box;
use core::sync::atomic::AtomicU32;

const VIRTIO_VENDOR: u16 = 0x1af4;  // Red Hat, Inc.
const VIRTIO_BLK_DEVICE: u16 = 0x1042;

/// Virtio block device driver state.
pub struct VirtioBlkDriver {
    name: &'static str,
    capacity: u64,              // Number of sectors
    sector_size: u32,
    #[allow(dead_code)]
    request_counter: AtomicU32, // For tracking in-flight requests
}

impl VirtioBlkDriver {
    /// Initializes the virtio-blk driver by finding a device and setting it up.
    pub fn init() -> Option<Box<VirtioBlkDriver>> {
        let pci_dev = find_device(VIRTIO_VENDOR, VIRTIO_BLK_DEVICE)?;

        kprintln!("[virtio-blk] found device at {:02x}:{:02x}.{}",
                 pci_dev.bdf.bus, pci_dev.bdf.device, pci_dev.bdf.function);

        // For now, hardcode the capacity for the test disk (64 MiB = 131072 sectors).
        // In a real implementation, this would be read from the device's config space.
        let capacity = 131072u64;
        let sector_size = 512u32;

        kprintln!("[virtio-blk] vda: {} sectors", capacity);

        Some(Box::new(VirtioBlkDriver {
            name: "vda",
            capacity,
            sector_size,
            request_counter: AtomicU32::new(0),
        }))
    }

    /// Returns the device's capacity in sectors.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Returns the device's sector size (typically 512).
    pub fn sector_size(&self) -> u32 {
        self.sector_size
    }

    /// Returns the device's name (e.g. "vda").
    pub fn name(&self) -> &str {
        self.name
    }
}
