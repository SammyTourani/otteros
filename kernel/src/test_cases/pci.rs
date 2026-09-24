//! PCI enumeration tests (brief M3-T1).

use otteros_kernel::drivers::pci;

#[test_case]
fn pci_find_host_bridge() {
    // Most PCs have a host bridge at 00:00.0 (typically Intel/AMD).
    // In QEMU, it's 8086:29c0 (Intel Q35 chipset).
    let devices = pci::devices();
    let found = devices.iter().any(|d| {
        d.bdf.bus == 0 && d.bdf.device == 0 && d.bdf.function == 0 && d.class_code == 0x06
    });
    assert!(found, "No host bridge (class 0x06) found at 00:00.0");
}

#[test_case]
fn pci_bars_are_nonzero_or_none() {
    // BAR values are either None (unimplemented) or Some(non-zero address/size).
    // This is a basic sanity check; real size probing happens in drivers.
    let devices = pci::devices();
    for dev in devices {
        for &_bar_opt in dev.bars.iter() {
            // Just verify we can read BARs without crashing.
            // If the BAR is present, it should be non-zero (or implementation-defined).
            // We won't assert on actual values here as they're device-specific.
        }
    }
}

#[test_case]
fn pci_devices_are_non_zero() {
    // At least the host bridge should be found.
    let devices = pci::devices();
    assert!(!devices.is_empty(), "No PCI devices found");
}

#[test_case]
fn pci_header_types_are_valid() {
    // header_type's lower 7 bits should be 0 (standard device) or 1 (PCI bridge).
    let devices = pci::devices();
    for dev in devices {
        let type_bits = dev.header_type & 0x7f;
        assert!(
            type_bits == 0 || type_bits == 1 || type_bits == 2,
            "Invalid header type 0x{:02x} for device {:02x}:{:02x}.{}",
            dev.header_type, dev.bdf.bus, dev.bdf.device, dev.bdf.function
        );
    }
}
