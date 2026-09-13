//! Architecture-specific primitives, gathered behind `arch::<target>::*` so
//! the rest of the kernel doesn't need `cfg(target_arch = ...)` sprinkled
//! around it (DECISIONS.md D1: x86_64 only for now).

pub mod x86_64;
