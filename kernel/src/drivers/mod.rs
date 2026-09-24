//! Device drivers that aren't core CPU/memory/interrupt plumbing
//! (`arch`/`mm`/`acpi`). Just the PS/2 keyboard for now (brief M1-T6,
//! DECISIONS.md D9); the mouse and later devices join here too.

pub mod ps2;
pub mod pci;
pub mod virtio;
