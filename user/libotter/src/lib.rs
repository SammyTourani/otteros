//! `libotter`: the OtterOS userspace runtime (brief M2-T3, DECISIONS.md
//! D18). Every user program links against this for its entry point
//! (`_start`), syscall wrappers, a global allocator backed by `map_anon`,
//! `print!`/`println!`/`eprintln!`/`read_line`, and process control.
#![no_std]

extern crate alloc;

mod alloc_impl;
pub mod io;
pub mod lineeditor;
pub mod mem;
pub mod panic;
pub mod process;
mod rt;
pub mod sync;
pub mod syscall;

pub use panic::set_program_name;
pub use process::{Pid, exit, getpid, kill, proc_list, reboot, sleep_ms, spawn, sysinfo, test_exit, time_ms, wait, yield_now};
pub use rt::args;
pub use syscall::Errno;
