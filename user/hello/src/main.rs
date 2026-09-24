//! `hello` (brief M2-T3 step 6): prints its own name and argv, exits 0.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;

/// The one entry point `libotter::rt` calls into (`#[unsafe(no_mangle)]`
/// so it links against `rt.rs`'s `extern "Rust" { fn main(...); }`
/// declaration by its exact, unmangled symbol name).
#[unsafe(no_mangle)]
pub fn main(args: &[&str]) -> i32 {
    libotter::set_program_name("hello");

    let mut joined = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            joined.push_str(", ");
        }
        joined.push_str(arg);
    }
    libotter::println!("hello from /bin/hello argv=[{joined}]");
    0
}
