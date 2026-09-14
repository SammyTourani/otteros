//! OtterOS kernel support library: shared by both kernel binaries (the
//! bootable `otteros-kernel` and the test harness `otteros-kernel-test`,
//! see src/main.rs and src/test_main.rs).
#![no_std]

// `alloc::{vec, string, boxed, collections}` (brief M1-T3), routed through
// `mm::heap::KernelHeap`, the `#[global_allocator]` it registers.
extern crate alloc;

use core::panic::PanicInfo;

use limine::{BaseRevision, RequestsEndMarker, RequestsStartMarker};

pub mod arch;
pub mod cmdline;
pub mod font8x8;
pub mod framebuffer;
pub mod mm;
pub mod qemu;
pub mod serial;
pub mod tests;

/// The Limine base revision we require (DECISIONS.md D3: base revision 3).
#[used]
#[unsafe(link_section = ".requests")]
pub static BASE_REVISION: BaseRevision = BaseRevision::with_revision(3);

/// Requests must sit between these two markers so Limine can find them
/// (see kernel/linker-x86_64.ld).
#[used]
#[unsafe(link_section = ".requests_start_marker")]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

/// Shared boot sequence for every kernel entry point: bring up serial,
/// confirm Limine speaks a base revision we understand, and announce
/// `[ok] boot` -- the line the test harness and humans both look for first.
pub fn init() {
    serial::init();
    if !BASE_REVISION.is_supported() {
        crate::kprintln!("[init] FATAL: limine base revision unsupported");
        qemu::exit(false);
    }
    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    mm::init();
    mm::heap::init();
    crate::kprintln!("[ok] boot");
}

/// Parks the CPU forever. The last thing every entry point does.
pub fn hlt_loop() -> ! {
    loop {
        // SAFETY: `hlt` halts the CPU until the next interrupt and is always
        // valid to execute from ring 0. Interrupts aren't enabled until M1,
        // so on real hardware this would in fact halt forever; under QEMU,
        // NMIs/reset from the harness are what actually end execution here.
        unsafe { core::arch::asm!("hlt", options(nomem, nostack, preserves_flags)) };
    }
}

// A panic can happen while normal code (or a fault handler) already holds
// `serial::SERIAL1`'s lock -- e.g. a bug in the formatting code `kprintln!`
// itself calls -- so, like `trap::trap_dispatch`, this uses the lock-free
// emergency writer, never the normal `kprintln!`. See `serial::EmergencyWriter`.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    match info.location() {
        Some(loc) => {
            crate::kprintln_emergency!("PANIC at {}:{}: {}", loc.file(), loc.line(), info.message())
        }
        None => crate::kprintln_emergency!("PANIC at <unknown location>: {}", info.message()),
    }
    qemu::exit(false);
}
