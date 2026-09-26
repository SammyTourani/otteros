#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

mod codec;
mod client;
mod server;

pub use client::ClientMsg;
pub use server::ServerMsg;

// Protocol constants
pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_STR: usize = 256;
pub const FLAG_RESIZABLE: u32 = 1;
pub const FLAG_UNDECORATED: u32 = 2;
pub const FLAG_DIALOG: u32 = 4;

/// Cursor shape enum for SetCursor message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Cursor {
    Arrow = 0,
    Text = 1,
    Hand = 2,
    ResizeH = 3,
    ResizeV = 4,
    Busy = 5,
}

/// Wire protocol errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    Truncated,
    Trailing,
    UnknownKind,
    BadUtf8,
    TooLong,
    BadValue,
}
