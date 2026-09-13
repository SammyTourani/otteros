//! The bootable OtterOS kernel: prints the serial boot banner, draws the
//! framebuffer banner, and halts. See src/test_main.rs for the test-mode
//! sibling binary (`gmake test` / `gmake bios-test` / `gmake panic-test`
//! all boot that one instead of this one).
#![no_std]
#![no_main]

use otteros_kernel::{cmdline, framebuffer, hlt_loop, init, kprintln};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    init();

    // Logged for forward-compatibility / debugging; this binary always shows
    // the banner regardless of cmdline (see brief M0-T1 "Deviations").
    let cmd = cmdline::get();
    if !cmd.is_empty() {
        kprintln!("[boot] cmdline: {}", cmd);
    }

    match framebuffer::get() {
        Some(fb) => {
            framebuffer::draw_banner(fb);
            kprintln!("[ok] fb banner");
        }
        None => kprintln!("[boot] WARNING: no framebuffer response from Limine"),
    }

    hlt_loop();
}
