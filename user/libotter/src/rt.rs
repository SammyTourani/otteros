//! `_start` (brief M2-T3 step 5): the naked entry point every OtterOS
//! process lands on. Reads argc/argv off the initial stack
//! (DECISIONS.md D18 / `proc::exec::build_initial_stack` on the kernel
//! side: `argc, argv[0..argc], NULL, envp NULL, auxv, strings`), aligns
//! `rsp`, and calls the program's own `main`.

use alloc::vec::Vec;

use crate::process;

// Every program built against this runtime defines exactly this function.
// Declared here as `extern "Rust"` so `rt_main` can call straight into
// whichever program this binary actually is, without `libotter` (a
// dependency *of* every program) ever depending on any one of them. Sound
// despite `extern "Rust"`'s otherwise-unspecified ABI: both sides are
// compiled by the exact same rustc invocation, with no FFI boundary to
// another compiler ever crossing this call -- the same "runtime crate
// declares `extern "Rust" fn main`, leaf crate defines `#[unsafe(no_mangle)]
// fn main`" shape common embedded-Rust runtimes already rely on. (Plain
// `//` comments, not `///`: rustdoc doesn't generate docs for `extern`
// blocks, and `///` there is a warned-on no-op.)
unsafe extern "Rust" {
    fn main(args: &[&str]) -> i32;
}

/// The process's own argv, stored once by `rt_main` before calling `main`
/// so `args()` (brief step 5's own function) can hand it back to code that
/// doesn't have it as a parameter. Never reassigned after that one write.
static mut ARGS: Vec<&'static str> = Vec::new();

/// The naked entry point every OtterOS process starts running at
/// (`user/link.ld`'s `ENTRY(_start)`). `rsp` arrives 16-byte aligned,
/// pointing at `argc` (D18) -- captured into `rdi` before this re-aligns
/// `rsp` itself (a defensive no-op if the kernel already left it exactly
/// so, per the brief's own "reads argc/argv from the stack, aligns"
/// wording) and `call`s into `rt_main`, an ordinary (non-naked) `extern
/// "C"` function that does everything else. `rt_main` never returns
/// (`process::exit`); the `ud2` is an inert safety net.
///
/// # Safety
/// Only ever reached as a process's genuine first instruction, with `rsp`
/// pointing at a kernel-built SysV argc/argv/envp/auxv prologue.
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "C" fn _start() {
    core::arch::naked_asm!("mov rdi, rsp", "and rsp, -16", "call {rt_main}", "ud2", rt_main = sym rt_main,);
}

/// The first ordinary Rust code a process runs: converts the kernel's raw
/// argc/argv into a `Vec<&str>`, hands it to the program's own `main`, and
/// exits with whatever `main` returns.
///
/// # Safety
/// `rsp` must point at a live `argc` immediately followed by `argc`
/// pointers to NUL-terminated, UTF-8 strings that stay valid for the
/// lifetime of this process (exactly what `_start` hands it, straight from
/// the kernel).
unsafe extern "C" fn rt_main(rsp: *const u64) -> ! {
    // SAFETY: forwarded from this function's own contract.
    let argc = unsafe { rsp.read() } as usize;
    let argv_ptr = unsafe { rsp.add(1) }.cast::<*const u8>();

    let mut args = Vec::with_capacity(argc);
    for i in 0..argc {
        // SAFETY: `argv[i]` (`i < argc`) is a NUL-terminated string the
        // kernel wrote onto this exact stack (`proc::exec::build_image`);
        // it stays mapped and unchanged for this process's entire
        // lifetime (nothing here or in the kernel ever touches that
        // region again).
        let ptr = unsafe { argv_ptr.add(i).read() };
        // SAFETY: `ptr` is genuinely NUL-terminated (see above), so
        // scanning for that NUL never reads past the end of the mapping.
        let cstr = unsafe { core::ffi::CStr::from_ptr(ptr.cast()) };
        args.push(core::str::from_utf8(cstr.to_bytes()).unwrap_or(""));
    }
    // SAFETY: this is the one and only write to `ARGS`, happening here,
    // once, before `main` (the only reader, via `args()`) ever runs --
    // OtterOS processes are single-threaded (DECISIONS.md D17), so no
    // concurrent access can ever race it.
    unsafe { ARGS = args };

    // SAFETY: every program linked against this runtime defines `main`
    // with exactly this signature (the `extern "Rust"` block above).
    let code = unsafe { main(args_slice()) };
    process::exit(code);
}

/// The process's own command-line arguments (brief step 5) -- `main`'s own
/// `args: &[&str]` parameter is the identical data; this exists for code
/// that doesn't have it as a parameter (e.g. a helper `main` doesn't pass
/// its own args down to).
pub fn args() -> &'static [&'static str] {
    args_slice()
}

fn args_slice() -> &'static [&'static str] {
    // SAFETY: `ARGS` is written exactly once (`rt_main`, above) before
    // `main` -- the only code that ever runs afterward -- starts; reading
    // it through a raw pointer rather than `&ARGS`/`&mut ARGS` sidesteps
    // needing a reference to a `static mut` at all, while relying on the
    // exact same "single-threaded, write-then-read-only" invariant.
    unsafe { &*core::ptr::addr_of!(ARGS) }
}
