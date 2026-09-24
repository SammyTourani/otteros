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
            // is dozens of lines), *then* the title and memory lines print.
            // `[ok] fb banner` itself has moved past the userspace-spawn
            // block below (brief M2-T3, see its own comment) so that
            // whatever it manages to print is on screen too, comfortably
            // within a screenful of the tail of the boot log, at the exact
            // moment `gmake shot` captures it -- rather than have any of
            // it scroll away under a backlog taller than the screen.
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

            // Brief M2-T3 step 7: once a real initramfs exists, boot
            // straight into userspace pid 1 instead of the M2-T1 typing
            // echo -- that thread only still runs as a fallback for a
            // kernel-only boot (no `/boot/initramfs.tar` module at all).
            //
            // Kernel-review-worthy, found the hard way: `gmake shot`
            // (`scripts/qemu.py`'s `cmd_shot`) takes its screendump the
            // instant `[ok] fb banner` appears in the serial log, then
            // tears QEMU down immediately -- there is no grace period
            // afterward, not even long enough for one more `kprintln!`.
            // `init` printing its own banner is therefore only ever
            // capturable if it happens *before* `[ok] fb banner` prints,
            // which is why that line has moved below this block instead
            // of above it (unlike the M1-T7/M2-T1 echo-thread case, where
            // the spawned thread's own output was never part of the shot
            // itself). Spawning is still non-blocking here -- `init`, in
            // normal mode, idles forever once there's nothing left to
            // spawn (no `/bin/sh` yet), so actually `wait`ing for it here
            // would mean this line, and the `hlt_loop` below, would never
            // be reached at all; a dedicated waiter thread (below) handles
            // "log it if init ever exits" instead, and this thread only
            // gives the scheduler a bounded, fixed number of chances to
            // actually run `init` (and, transitively, `/bin/hello`) before
            // moving on regardless of whether either has finished.
            if otteros_kernel::fs::initramfs::is_present() {
                match otteros_kernel::proc::spawn("/bin/init", &[]) {
                    Ok(pid) => {
                        otteros_kernel::sched::spawn("init-waiter", wait_for_init_exit, pid as usize);
                        for _ in 0..40 {
                            otteros_kernel::sched::yield_now();
                        }
                    }
                    Err(e) => kprintln!("[proc] FATAL: could not spawn /bin/init: {e:?}"),
                }
            } else if keyboard::is_available() {
                kprintln!("[kbd] keyboard ready: type on the QEMU window to try it");
                kprint!("> ");
                otteros_kernel::sched::spawn("echo", run_typing_echo, 0);
            }

            kprintln!("[ok] fb banner");
        }
        None => kprintln!("[boot] WARNING: no framebuffer response from Limine"),
    }

    hlt_loop();
}

/// Runs as its own kernel thread (`sched::spawn`, brief M2-T3): blocks on
/// `wait(pid)` for as long as `/bin/init` (`pid`, packed into a `usize`
/// for `ThreadEntry`'s signature) runs -- which, in normal mode, can be
/// forever (see `after_vmm`'s own docs) -- and logs the brief's own
/// "`[proc] init exited with <code>`" line if it ever actually does.
fn wait_for_init_exit(pid: usize) -> i32 {
    let code = otteros_kernel::proc::wait(pid as u64).unwrap_or(-1);
    kprintln!("[proc] init exited with {code}");
    0
}

/// Echoes every decoded character to serial and to the console, forever --
/// so a human running `gmake run` can try the keyboard end to end.
/// `Console`'s own newline/backspace/scrolling (brief M1-T7) does all the
/// line-editing work now; this just forwards decoded characters to it via
/// the ordinary `kprint!` path. Runs as a spawned kernel thread (brief
/// M2-T1); the `-> i32` and unused `usize` argument are `sched::spawn`'s
/// thread entry-point signature -- this loop never actually returns one.
/// The startup message and first prompt print before this thread is even
/// spawned (see `after_vmm`); this is purely the blocking read loop.
fn run_typing_echo(_arg: usize) -> i32 {
    loop {
        match keyboard::read_char_blocking() {
            '\n' => kprint!("\n> "),
            '\u{8}' => kprint!("\u{8}"),
            c if c.is_ascii() && !c.is_ascii_control() => kprint!("{c}"),
            _ => {}
        }
    }
}
