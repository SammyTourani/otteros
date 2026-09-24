//! PCI enumeration via ACPI MCFG/ECAM (brief M3-T1): discover PCI devices
//! on the bus, parse their configuration headers, and collect BAR and
//! capability information. DECISIONS.md D2: no `pci` or `x86_64` crates.

pub mod ecam;
pub mod config;

use alloc::vec::Vec;
use crate::acpi;
use crate::kprintln;
use crate::mm::addr::PhysAddr;
use crate::sync::IrqMutex;

/// A PCI device identifier: bus, device, function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bdf {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl Bdf {
    pub fn new(bus: u8, device: u8, function: u8) -> Self {
        Bdf { bus, device, function }
    }

    fn ecam_offset(&self) -> u32 {
        ((self.bus as u32) << 20) | ((self.device as u32) << 15) | ((self.function as u32) << 12)
    }
}

#[derive(Clone, Debug)]
pub struct PciDevice {
    pub bdf: Bdf,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass_code: u8,
    pub prog_if: u8,
    pub header_type: u8,
    pub bars: [Option<u64>; 6],
    pub bar_is_io: [bool; 6],
    pub capabilities: alloc::vec::Vec<(u8, u8)>, // (capability ID, offset)
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

impl PciDevice {
    /// Returns true if this device is a multifunction device.
    fn is_multifunction(&self) -> bool {
        (self.header_type & 0x80) != 0
    }
}

static PCI_DEVICES: IrqMutex<Vec<PciDevice>> = IrqMutex::new(Vec::new());

/// Enumerate PCI devices by walking the MCFG table and recursively
/// scanning buses (bridges). Call once at boot before driver init.
pub fn init() {
    acpi::with_info(|info| {
        match &info.mcfg {
            Some(mcfg) => {
                for alloc in &mcfg.allocations {
                    scan_segment(alloc.segment, alloc.bus_start, alloc.bus_end, alloc.base_addr);
                }
            }
            None => {
                kprintln!("[pci] MCFG not found in ACPI tables");
            }
        }
    });

    // Print discovered devices.
    let devices = PCI_DEVICES.lock();
    for dev in devices.iter() {
        let type_str = match dev.class_code {
            0x01 => "storage",
            0x02 => "network",
            0x03 => "display",
            0x06 => "bridge",
            _ => "other",
        };
        let subclass = match (dev.class_code, dev.subclass_code) {
            (0x01, 0x00) => "scsi",
            (0x01, 0x01) => "ide",
            (0x01, 0x08) => "nvme",
            _ => "?",
        };

        // Log: [pci] 00:03.0 1af4:1042 class 01.00 <subclass>
        // (1af4 is Red Hat/QEMU vendor ID, 1042 is virtio-blk)
        kprintln!(
            "[pci] {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}.{:02x} {}-{}",
            dev.bdf.bus,
            dev.bdf.device,
            dev.bdf.function,
            dev.vendor_id,
            dev.device_id,
            dev.class_code,
            dev.subclass_code,
            type_str,
            subclass
        );
    }
}

fn scan_segment(_segment: u16, bus_start: u8, bus_end: u8, base_addr: PhysAddr) {
    for bus in bus_start..=bus_end {
        scan_bus(bus, base_addr);
    }
}

fn scan_bus(bus: u8, base_addr: PhysAddr) {
    for device in 0..32 {
        let bdf = Bdf::new(bus, device, 0);
        if let Some(hdr) = read_device_header(base_addr, bdf) {
            let is_mf = hdr.is_multifunction();
            process_device(base_addr, hdr);
            // Check if it's a multifunction device.
            if is_mf {
                for func in 1..8 {
                    let bdf = Bdf::new(bus, device, func);
                    if let Some(hdr) = read_device_header(base_addr, bdf) {
                        process_device(base_addr, hdr);
                    }
                }
            }
        }
    }
}

fn read_device_header(base_addr: PhysAddr, bdf: Bdf) -> Option<PciDevice> {
    let header = config::read_header(base_addr, bdf);
    if header.vendor_id == 0xffff || header.vendor_id == 0 {
        return None;
    }

    let mut caps = alloc::vec::Vec::new();
    // Scan capability list if status indicates capabilities present (bit 4).
    if (header.status & 0x10) != 0 {
        caps = config::scan_capabilities(base_addr, bdf);
    }

    Some(PciDevice {
        bdf,
        vendor_id: header.vendor_id,
        device_id: header.device_id,
        class_code: header.class_code,
        subclass_code: header.subclass_code,
        prog_if: header.prog_if,
        header_type: header.header_type,
        bars: header.bars,
        bar_is_io: header.bar_is_io,
        capabilities: caps,
        interrupt_line: header.interrupt_line,
        interrupt_pin: header.interrupt_pin,
    })
}

fn process_device(base_addr: PhysAddr, dev: PciDevice) {
    // If it's a bridge, recursively scan the secondary bus.
    if (dev.header_type & 0x7f) == 0x01 {
        // PCI-to-PCI bridge
        let secondary_bus = config::read_bridge_secondary_bus(base_addr, dev.bdf);
        if secondary_bus != 0xff {
            scan_bus(secondary_bus, base_addr);
        }
    }

    PCI_DEVICES.lock().push(dev);
}

/// Returns the list of discovered PCI devices.
pub fn devices() -> Vec<PciDevice> {
    PCI_DEVICES.lock().clone()
}

/// Finds the first PCI device matching the given vendor and device IDs.
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    PCI_DEVICES.lock().iter().find(|d| d.vendor_id == vendor_id && d.device_id == device_id).cloned()
}
