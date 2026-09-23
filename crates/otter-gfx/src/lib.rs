//! `otter-gfx`: OtterOS's pure 2D graphics, PNG codec and TrueType text engine.
//!
//! DECISIONS.md D27: this crate is `#![no_std]` + `alloc`, has zero external
//! dependencies, and is unit-tested on the host with `cargo test`. It contains no
//! OS logic (D2) -- only computation over byte slices and in-memory buffers -- and
//! is linked into userspace programs by path once userspace exists.
//!
//! `std` is enabled only under `cfg(test)`, purely so unit/integration tests can do
//! file I/O (loading font/PNG fixtures, writing the specimen image). It adds no
//! Cargo dependency and the crate is never built with `std` outside test builds.
#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

extern crate alloc;

pub mod checksum;
pub mod color;
pub mod deflate;
pub mod font;
pub mod geom;
pub mod gradient;
pub mod inflate;
pub mod path;
pub mod png;
pub mod raster;
pub mod shadow;
pub mod shapes;
pub mod surface;
pub mod text;

pub use color::Color;
pub use geom::{Affine, Point, Rect};
pub use path::Path;
pub use surface::{Canvas, Surface};
