//! Virtio 1.x modern PCI transport driver (brief M3-T1b).
//! Virtio is a hypervisor-agnostic I/O interface for VMs. Devices
//! (like virtio-blk) expose their configuration through PCI capabilities.

pub mod blk;

pub use blk::VirtioBlk;
