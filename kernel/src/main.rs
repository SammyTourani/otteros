//! The bootable OtterOS kernel: prints the serial boot banner, draws the
//! framebuffer banner, and halts. See src/test_main.rs for the test-mode
//! sibling binary (`gmake test` / `gmake bios-test` / `gmake panic-test`
//! all boot that one instead of this one).
#![no_std]
#![no_main]

use otteros_kernel::mm::addr::FRAME_SIZE;
use otteros_kernel::mm::pmm;
use otteros_kernel::{cmdline, framebuffer, hlt_loop, init, kprintln};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // SAFETY: the very first thing this kernel does, before relying on any
    // assumption about the interrupt flag Limine/firmware left us with
    // (kernel-review, M1-T1); disabling interrupts is always valid from
    // ring 0.
    unsafe { core::arch::asm!("cli", options(nomem, nostack, preserves_flags)) };

    init(after_vmm)
}

/// Runs on the kernel's own guard-paged stack, after `init()` has built
/// the kernel's page tables, switched CR3 and moved off the stack Limine
/// handed it at entry (brief M1-T4) -- the rest of the M0/M1 boot banner
/// sequence, unchanged from before that switch existed.
extern "C" fn after_vmm() -> ! {
    // Logged for forward-compatibility / debugging; this binary always shows
    // the banner regardless of cmdline (see brief M0-T1 "Deviations").
    let cmd = cmdline::get();
    if !cmd.is_empty() {
        kprintln!("[boot] cmdline: {}", cmd);
    }

    match framebuffer::get() {
        Some(fb) => {
            framebuffer::draw_banner(fb);
            let stats = pmm::stats();
            let frames_per_mib = (1024 * 1024) / FRAME_SIZE as u64;
            framebuffer::draw_mem_line(
                fb,
                stats.total as u64 / frames_per_mib,
                stats.free as u64 / frames_per_mib,
            );
            kprintln!("[ok] fb banner");
        }
        None => kprintln!("[boot] WARNING: no framebuffer response from Limine"),
    }

    hlt_loop();
}
