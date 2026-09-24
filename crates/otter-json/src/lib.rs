//! `otter-json`: OtterOS's own JSON, written from scratch.
//!
//! A strict RFC 8259 parser to an order-preserving DOM ([`Value`]), plus a
//! serializer (compact and pretty). No trailing commas, no comments, full
//! string escapes (including surrogate pairs), a byte-offset on every parse
//! error, and a nesting depth limit so pathological input fails fast instead
//! of recursing deeply. DECISIONS.md D27: `#![no_std]` + `alloc`, zero
//! external dependencies, unit-tested on the host with `cargo test` --
//! including against the upstream JSONTestSuite corpus (see `tests/`).
//!
//! Two consumers: `tools/otter-convert` reads Hugging Face `config.json` /
//! `tokenizer.json` / `tokenizer_config.json` through this crate (brief
//! M7-T1), and from M9 it builds the Claude API's JSON request/response
//! bodies.
//!
//! Always `#![no_std]` -- unlike some sibling crates, nothing here needs a
//! `cfg(test)` escape hatch to `std`, because all of this crate's tests are
//! black-box integration tests (`tests/`): plain `std` binaries that link
//! against this library and need no special build mode to do file I/O or
//! timing.
#![no_std]
#![warn(missing_docs)]

extern crate alloc;

mod error;
mod parser;
mod ser;
mod value;

pub use error::{Error, ErrorKind};
pub use parser::{MAX_DEPTH, parse, parse_str};
pub use ser::{to_string, to_string_pretty, to_string_pretty_with_indent, write_compact, write_pretty};
pub use value::{Number, Object, Value};
