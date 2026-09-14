//! OtterOS kernel support library: shared by both kernel binaries (the
//! bootable `otteros-kernel` and the test harness `otteros-kernel-test`,
//! see src/main.rs and src/test_main.rs).
#![no_std]

// `alloc::{vec, string, boxed, collections}` (brief M1-T3), routed through
// `mm::heap::KernelHeap`, the `#[global_allocator]` it registers.
extern crate alloc;

use core::panic::PanicInfo;

use limine::{BaseRevision, RequestsEndMarker, RequestsStartMarker};

pub mod acpi;
pub mod arch;
pub mod cmdline;
pub mod drivers;
pub mod font8x8;
pub mod framebuffer;
pub mod mm;
pub mod qemu;
pub mod serial;
pub mod sync;
pub mod tests;
pub mod time;

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

/// The boot stack `init` allocates and switches to (brief M1-T4 step 4):
/// 64 KiB, matching the placeholder `KERNEL_STACK` `gdt::init()` used
/// before this ran.
const BOOT_STACK_PAGES: usize = 64 * 1024 / mm::addr::FRAME_SIZE;

/// Shared boot sequence for every kernel entry point: brings up serial,
/// confirms Limine speaks a base revision we understand, announces
/// `[ok] boot`, then builds the kernel's own page tables and switches
/// onto a guard-paged kernel stack (brief M1-T4) -- and, having replaced
/// `rsp` out from under this function's own frame, never returns:
/// `continue_boot` (each entry point's own `after_vmm`) is how execution
/// keeps going instead.
pub fn init(continue_boot: extern "C" fn() -> !) -> ! {
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

    mm::vmm::init_kernel_space();
    let stack = mm::kstack::init_boot_stack(BOOT_STACK_PAGES);
    // SAFETY: `stack.top` was just mapped by `init_boot_stack` above as an
    // exclusively-owned, `BOOT_STACK_PAGES`-frame stack, and `continue_boot`
    // is `-> !` by this function's own signature.
    unsafe { mm::kstack::switch_stack_and_call(stack.top.as_u64(), continue_boot) }
}

/// The interrupt-subsystem half of the shared boot sequence (brief
/// M1-T5): discovers the machine from ACPI, programs the legacy PIC
/// (remapped off the exception vectors, then fully masked) and the
/// local/IO APICs, calibrates and programs the LAPIC timer at 1 kHz, and
/// finally `sti`s -- for good, from this point on nothing in this kernel
/// ever `cli`s again except inside a critical section a `sync::IrqMutex`
/// guard (or `arch::x86_64::interrupts::without_interrupts`) already
/// owns.
///
/// Kept separate from `init()` itself (rather than folded into it) because
/// every register here is MMIO reached through the HHDM, which only
/// covers these fixed hardware addresses (the LAPIC, the I/O APIC, ...)
/// once the kernel's own page tables are active -- `init()` only builds
/// those and switches onto the guard-paged boot stack, it doesn't run
/// anything past that switch itself (see `mm::kstack::switch_stack_and_call`).
/// Each binary's own `continue_boot` (`after_vmm` in main.rs/test_main.rs)
/// calls this itself, as the very first thing it does once running on
/// that new stack.
///
/// `drivers::ps2::init` (brief M1-T6) runs last, after `irq::enable`'s
/// `sti`: its i8042 bring-up sequence bounds every wait against
/// `time::ticks()`, which only advances once interrupts are actually on.
/// IRQ1 itself stays masked at the I/O APIC (`arch::x86_64::ioapic::init`
/// already masked every entry) until that init sequence explicitly
/// unmasks it as its very last step, so nothing here can race a keyboard
/// interrupt against its own not-yet-registered handler.
pub fn start_interrupts() {
    acpi::init();
    arch::x86_64::pic::remap_and_mask();
    arch::x86_64::lapic::init();
    arch::x86_64::ioapic::init();
    time::init();
    arch::x86_64::irq::enable();
    drivers::ps2::init();
}

/// Parks the CPU forever. The last thing every entry point does.
pub fn hlt_loop() -> ! {
    loop {
        // SAFETY: `hlt` halts the CPU until the next interrupt and is
        // always valid to execute from ring 0. By the time anything calls
        // this, `start_interrupts` has already `sti`d for good (brief
        // M1-T5), so the next LAPIC timer tick (or any other interrupt)
        // always wakes this back up -- harmlessly, since there's nothing
        // to do but immediately `hlt` again until a real scheduler (a
        // later milestone) gives this loop something else to run instead.
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
