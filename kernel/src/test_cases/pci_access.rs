//! PCI configuration-access tests for brief M3-T1d, written by the orchestrator: the
//! implementation must pass these unchanged. They run in the normal suite on `-M q35` (ECAM from
//! ACPI MCFG, and the legacy 0xCF8/0xCFC mechanism is present too) and in `gmake pc-test` on
//! `-M pc` (i440FX: no MCFG, so the kernel must fall back to port I/O).
//!
//! API: `pci::access_method() -> AccessMethod::{Ecam, PortIo}`, `pci::port_io_read32(Bdf, u8)`
//! (mechanism #1, always available, dword-aligned offsets), `pci::config_read32(Bdf, u16)` (the
//! active method; offsets >= 256 read 0xFFFF_FFFF under port I/O), `acpi::has_mcfg() -> bool`,
//! plus the existing `pci::devices()` and `Bdf`.

use otteros_kernel::acpi;
use otteros_kernel::drivers::pci::{self, AccessMethod, Bdf};

#[test_case]
fn pci_access_method_matches_the_firmware() {
    let expected = if acpi::has_mcfg() { AccessMethod::Ecam } else { AccessMethod::PortIo };
    assert_eq!(pci::access_method(), expected);
}

#[test_case]
fn pci_port_io_sees_the_host_bridge() {
    let id = pci::port_io_read32(Bdf::new(0, 0, 0), 0);
    assert_eq!(id & 0xFFFF, 0x8086, "00:00.0 is an Intel host bridge on both q35 and i440FX: {id:#010x}");
    assert_eq!(pci::port_io_read32(Bdf::new(0, 16, 0), 0), 0xFFFF_FFFF, "an empty slot reads all ones");
    assert_eq!(pci::port_io_read32(Bdf::new(255, 31, 7), 0), 0xFFFF_FFFF, "so does a missing bus");
}

#[test_case]
fn pci_mechanisms_agree_on_every_device() {
    let devices = pci::devices();
    assert!(devices.len() >= 3, "host bridge, virtio-blk, virtio-net at least");
    for dev in &devices {
        for offset in (0..0x40u8).step_by(4) {
            let legacy = pci::port_io_read32(dev.bdf, offset);
            let active = pci::config_read32(dev.bdf, offset as u16);
            // The status register (upper half of 0x04) has bits the device may flip between two
            // reads (e.g. interrupt status); compare only the stable command half there.
            let mask = if offset == 0x04 { 0x0000_FFFF } else { 0xFFFF_FFFF };
            assert_eq!(legacy & mask, active & mask, "{:?} offset {offset:#04x}", dev.bdf);
        }
    }
    if pci::access_method() == AccessMethod::PortIo {
        let first = &devices[0];
        assert_eq!(pci::config_read32(first.bdf, 0x100), 0xFFFF_FFFF, "no extended config space via port I/O");
    }
}

#[test_case]
fn pci_virtio_devices_are_found_either_way() {
    let devices = pci::devices();
    let virtio: usize = devices.iter().filter(|d| d.vendor_id == 0x1AF4).count();
    assert!(virtio >= 2, "virtio-blk and virtio-net, found through {:?}", pci::access_method());
}
