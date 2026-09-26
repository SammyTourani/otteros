//! `#[test_case]`s for the `otteros-kernel-test` binary (see
//! `src/test_main.rs`), split into one file per subsystem now that they
//! outgrew a single ~600-line module (brief M1-T3). `custom_test_frameworks`
//! collects `#[test_case]` items crate-wide, so nesting them under
//! submodules here changes nothing about how `test_main()` finds them --
//! only where the source lives.
//!
//! Brief M2-T2b: execution order is sorted by fully qualified name
//! (`module::function`), globally across the crate -- confirmed
//! empirically, not just declaration order within a file, *and*
//! independent of the order `mod` items are listed below. A test that
//! needs to run early (`keyboard_e2e`: `scripts/qemu.py --send-keys`
//! waits on serial for it, under an overall timeout) therefore has to
//! live in a module whose *name* sorts early, not just be declared first
//! here.

mod acpi;
mod block;
mod console;
mod elf;
mod heap;
mod initramfs;
mod irq;
mod keyboard_e2e;
mod mm;
mod net;
mod net_udp;
mod payloads;
mod pci;
mod proc;
mod ps2;
mod random;
mod sched;
mod timer;
mod traps;
mod virtio_blk;
mod vmm;
mod zz_spawn;
