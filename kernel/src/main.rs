//! The bootable OtterOS kernel: brings up the framebuffer console (brief
//! M1-T7), prints a title/memory banner, replays the boot log logged
//! before the console existed, and then hands off to a small typing echo
//! forever. See src/test_main.rs for the test-mode sibling binary
//! (`gmake test` / `gmake bios-test` / `gmake panic-test` all boot that
//! one instead of this one).
#![no_std]
#![no_main]

use otteros_kernel::drivers::ps2::keyboard;
use otteros_kernel::mm::addr::FRAME_SIZE;
use otteros_kernel::mm::pmm;
use otteros_kernel::{cmdline, console, framebuffer, hlt_loop, init, kprint, kprintln};

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
/// handed it at entry (brief M1-T4) -- the rest of the M0/M1 boot
/// sequence, unchanged from before that switch existed.
extern "C" fn after_vmm() -> ! {
    otteros_kernel::start_interrupts();

    // Logged for forward-compatibility / debugging; this binary always shows
    // the console regardless of cmdline (see brief M0-T1 "Deviations").
    let cmd = cmdline::get();
    if !cmd.is_empty() {
        kprintln!("[boot] cmdline: {}", cmd);
    }

    match framebuffer::get() {
        Some(fb) => {
            // Brief M1-T7 step 4: `replay_boot_log` catches the console up
            // on everything `init`/`start_interrupts` already logged
            // before it existed (comfortably more than a screenful on a
            // typical resolution -- the PMM's own memory-map dump alone
            // is dozens of lines), *then* the title and memory lines
            // print, right before `[ok] fb banner`. That keeps the
            // banner -- and a screenful of the tail of the boot log above
            // it -- on screen at the exact moment `gmake shot` captures
            // it, rather than have the title scroll away under a backlog
            // taller than the screen.
            console::init(framebuffer::Device::new(fb));
            console::replay_boot_log();

            kprintln!("\u{1b}[1mOtterOS\u{1b}[0m");

            let stats = pmm::stats();
            let frames_per_mib = (1024 * 1024) / FRAME_SIZE as u64;
            kprintln!(
                "MEM: {} MiB total, {} MiB free",
                stats.total as u64 / frames_per_mib,
                stats.free as u64 / frames_per_mib
            );

            kprintln!("[ok] fb banner");

            // brief M1-T6 step 5 / M1-T7 step 4: once a keyboard is
            // present, hand off to a small typing echo forever (a human
            // running `gmake run` can try it), now through the console
            // instead of drawing directly onto the framebuffer;
            // otherwise fall through to `hlt_loop` below.
            if keyboard::is_available() {
                run_typing_echo();
            }
        }
        None => kprintln!("[boot] WARNING: no framebuffer response from Limine"),
    }

    hlt_loop();
}

/// Echoes every decoded character to serial and to the console, forever --
/// so a human running `gmake run` can try the keyboard end to end.
/// `Console`'s own newline/backspace/scrolling (brief M1-T7) does all the
/// line-editing work now; this just forwards decoded characters to it via
/// the ordinary `kprint!` path.
fn run_typing_echo() -> ! {
    kprintln!("[kbd] keyboard ready: type on the QEMU window to try it");
    kprint!("> ");

    loop {
        match keyboard::read_char_blocking() {
            '\n' => kprint!("\n> "),
            '\u{8}' => kprint!("\u{8}"),
            c if c.is_ascii() && !c.is_ascii_control() => kprint!("{c}"),
            _ => {}
        }
    }
}
