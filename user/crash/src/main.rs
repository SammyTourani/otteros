//! `crash <kind>` (brief M2-T3 step 6): deliberately performs an
//! unrecoverable ring-3 fault, for `utest` (and a human) to confirm the
//! kernel kills only the offending process (`proc::fault`).
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub fn main(args: &[&str]) -> i32 {
    libotter::set_program_name("crash");

    match args.first().copied() {
        Some("null") => {
            // SAFETY: deliberately reads through a null pointer -- the
            // entire point of this branch is to be killed by the kernel
            // for it (a page fault at address 0).
            let ptr = core::ptr::null::<u8>();
            unsafe { core::ptr::read_volatile(ptr) };
        }
        Some("div0") => {
            // A genuine hardware `#DE`: Rust's `/`/`%` on integers always
            // check for a zero divisor and panic in *software* -- in every
            // build profile, not just debug (unlike overflow checks, this
            // one is never optimised away) -- so it can never actually
            // reach the CPU's own `div` with a zero divisor. Executing
            // `div` directly is the only way to trigger the real fault.
            //
            // SAFETY: `edx:eax / ecx` with `ecx = 0` is specifically
            // guaranteed to raise `#DE` (Intel SDM Vol. 1 3.2, `DIV`) --
            // the entire point of this branch is to be killed by the
            // kernel for it; `noreturn` is accurate because the CPU never
            // retires this instruction with `ecx = 0`.
            unsafe { core::arch::asm!("xor edx, edx", "mov eax, 1", "xor ecx, ecx", "div ecx", options(noreturn)) };
        }
        Some("ud") => {
            // SAFETY: `ud2` is x86's dedicated "always-invalid opcode"
            // instruction -- the entire point of this branch is to be
            // killed by the kernel for executing it.
            unsafe { core::arch::asm!("ud2", options(noreturn)) };
        }
        _ => {
            libotter::eprintln!("usage: crash <null|div0|ud>");
            return 1;
        }
    }

    // Unreachable if the fault above actually killed the process (the
    // expected outcome, checked on the *kernel* side via `wait`'s exit
    // code) -- reachable only as a bug-reporting fallback.
    libotter::eprintln!("crash: did not actually crash");
    2
}
