//! Virtio 1.x modern PCI transport driver (brief M3-T1).
//! Virtio is a hypervisor-agnostic I/O interface for VMs. Devices
//! (like virtio-blk) expose their configuration through PCI capabilities.

pub mod pci_transport;
pub mod queue;
pub mod blk;

pub use blk::VirtioBlkDriver;
pub use queue::VirtQueue;
