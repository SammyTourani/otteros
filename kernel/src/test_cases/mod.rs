//! `#[test_case]`s for the `otteros-kernel-test` binary (see
//! `src/test_main.rs`), split into one file per subsystem now that they
//! outgrew a single ~600-line module (brief M1-T3). `custom_test_frameworks`
//! collects `#[test_case]` items crate-wide, so nesting them under
//! submodules here changes nothing about how `test_main()` finds them --
//! only where the source lives.

mod acpi;
mod heap;
mod irq;
mod mm;
mod timer;
mod traps;
mod vmm;
