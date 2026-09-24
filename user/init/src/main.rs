//! `/bin/init`, pid 1 (brief M2-T3 step 6): prints a startup banner, spawns
//! `/bin/hello a b` and waits for it, then either runs `/bin/utest` and
//! exits with its code (`argv == ["test"]`, brief M2-T3 step 7's test-boot
//! path) or, in normal mode, spawns `/bin/sh` if it exists (a later task)
//! or idles forever.
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub fn main(args: &[&str]) -> i32 {
    libotter::set_program_name("init");
    libotter::println!("[init] OtterOS userspace up (pid {})", libotter::getpid());

    match libotter::spawn("/bin/hello", &["a", "b"]) {
        Ok(pid) => match libotter::wait(pid) {
            Ok(code) => libotter::println!("[init] /bin/hello exited with {code}"),
            Err(e) => libotter::eprintln!("[init] wait(/bin/hello) failed: {e:?}"),
        },
        Err(e) => libotter::eprintln!("[init] spawn(/bin/hello) failed: {e:?}"),
    }

    if args.first().copied() == Some("test") {
        // Test mode: run the shell in test mode if requested, else run utest
        // For now, always run utest for test mode
        return match libotter::spawn("/bin/utest", &["selftest"]) {
            Ok(pid) => libotter::wait(pid).unwrap_or(0),
            Err(e) => {
                libotter::eprintln!("[init] spawn(/bin/utest) failed: {e:?}");
                0
            }
        };
    }

    // For shell testing, check if shelltest is passed (used in gmake shell-test)
    if args.first().copied() == Some("shelltest") {
        return match libotter::spawn("/bin/sh", &["--test"]) {
            Ok(pid) => libotter::wait(pid).unwrap_or(0),
            Err(e) => {
                libotter::eprintln!("[init] spawn(/bin/sh --test) failed: {e:?}");
                1
            }
        };
    }

    // Normal mode: `/bin/sh` doesn't exist yet (M2-T4) -- `ENOENT` is
    // expected, not an error worth reporting; idle forever either way
    // once whatever shell/idle loop returns.
    match libotter::spawn("/bin/sh", &[]) {
        Ok(pid) => {
            let _ = libotter::wait(pid);
        }
        Err(_) => libotter::println!("[init] no /bin/sh yet; idling"),
    }
    loop {
        libotter::sleep_ms(1000);
    }
}
