//! `testloop` - a simple binary that loops forever, used by kernel tests
//! that need a spawned process to remain alive while its memory is inspected.
//! Brief M2-T4a race condition fix: keep spawned process runnable indefinitely.
#![no_std]
#![no_main]

/// Entry point - just loop forever, yielding to avoid busy-waiting.
#[unsafe(no_mangle)]
pub fn main(_args: &[&str]) -> i32 {
    libotter::set_program_name("testloop");
    // Never exit; just yield forever. This keeps the process in the ready queue
    // so it can be inspected by kernel tests without exiting and being reaped.
    loop {
        libotter::yield_now();
    }
}
