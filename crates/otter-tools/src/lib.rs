#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

mod format;
mod os;
mod path;
mod policy;
mod session;
mod spec;

pub use os::*;
pub use path::*;
pub use session::*;
pub use spec::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Png(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    AutoRun,
    Allowed,
    Denied,
    Refused,
    Invalid,
    Failed,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub tool: String,
    pub summary: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub tool: String,
    pub summary: String,
    pub detail: String,
}
