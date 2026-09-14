//! The bootable OtterOS kernel: prints the serial boot banner, draws the
//! framebuffer banner, and halts. See src/test_main.rs for the test-mode
//! sibling binary (`gmake test` / `gmake bios-test` / `gmake panic-test`
//! all boot that one instead of this one).
#![no_std]
#![no_main]

use limine::framebuffer::Framebuffer;
use otteros_kernel::drivers::ps2::keyboard;
use otteros_kernel::mm::addr::FRAME_SIZE;
use otteros_kernel::mm::pmm;
use otteros_kernel::{cmdline, framebuffer, hlt_loop, init, kprint, kprintln};

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
    otteros_kernel::start_interrupts();

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

            // brief M1-T6 step 5: once a keyboard is present, hand off to
            // a small typing echo forever (a human running `gmake run`
            // can try it); otherwise fall through to `hlt_loop` below.
            if keyboard::is_available() {
                run_typing_echo(fb);
            }
        }
        None => kprintln!("[boot] WARNING: no framebuffer response from Limine"),
    }

    hlt_loop();
}

/// How many typed characters `run_typing_echo` keeps on its one editable
/// line before it silently stops accepting more (until Enter or
/// Backspace makes room again).
const LINE_CAPACITY: usize = 60;

/// Echoes every decoded character to serial and to a one-line "> " prompt
/// on the framebuffer, forever -- so a human running `gmake run` can try
/// the keyboard end to end. No scrolling console yet (that's a later M1
/// item); Enter just starts a new, empty line.
fn run_typing_echo(fb: &Framebuffer) -> ! {
    const PROMPT_Y: u64 = 64;
    const LINE_HEIGHT: u64 = 8;

    kprintln!("[kbd] keyboard ready: type on the QEMU window to try it");

    let mut line = [0u8; LINE_CAPACITY];
    let mut len = 0usize;

    loop {
        draw_prompt_line(fb, PROMPT_Y, LINE_HEIGHT, &line[..len]);
        match keyboard::read_char_blocking() {
            '\n' => {
                kprintln!();
                len = 0;
            }
            '\u{8}' => len = len.saturating_sub(1),
            c if c.is_ascii() && !c.is_ascii_control() && len < LINE_CAPACITY => {
                line[len] = c as u8;
                len += 1;
                kprint!("{c}");
            }
            _ => {}
        }
    }
}

/// Redraws `run_typing_echo`'s one-line prompt: blanks the row, then
/// draws `"> "` followed by `line`.
fn draw_prompt_line(fb: &Framebuffer, y: u64, height: u64, line: &[u8]) {
    const BG: (u8, u8, u8) = (0x10, 0x14, 0x1c);
    const FG: (u8, u8, u8) = (0xe0, 0xe0, 0xe0);

    framebuffer::clear_rect(fb, 0, y, fb.width, height, BG);

    let mut buf = [0u8; 2 + LINE_CAPACITY];
    buf[0] = b'>';
    buf[1] = b' ';
    buf[2..2 + line.len()].copy_from_slice(line);
    if let Ok(text) = core::str::from_utf8(&buf[..2 + line.len()]) {
        framebuffer::draw_str(fb, 16, y, text, FG.0, FG.1, FG.2);
    }
}
