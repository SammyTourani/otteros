use alloc::string::String;
use alloc::vec::Vec;

/// A directory entry from `list_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Output from a `run_command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub output: Vec<u8>,
    pub truncated: bool,
}

/// Output from a `fetch` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutput {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub truncated: bool,
}

/// An application the agent can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum App {
    Terminal,
    Editor,
    Files,
    Otter,
}

/// Errors that can occur during OS operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsError {
    NotFound,
    NotADirectory,
    IsADirectory,
    NoSpace,
    Timeout,
    Io(String),
}

/// The OS interface that agents interact with through tools.
pub trait Os {
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, OsError>;
    fn read_file(&mut self, path: &str, limit: usize) -> Result<(Vec<u8>, u64), OsError>;
    fn file_size(&mut self, path: &str) -> Result<Option<u64>, OsError>;
    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), OsError>;
    fn run_command(&mut self, command: &str, timeout_ms: u64, limit: usize) -> Result<CommandOutput, OsError>;
    fn open_app(&mut self, app: App, path: Option<&str>) -> Result<(), OsError>;
    fn fetch(&mut self, url: &str, limit: usize) -> Result<FetchOutput, OsError>;
    fn screenshot(&mut self) -> Result<Vec<u8>, OsError>;
    fn ask(&mut self, approval: &super::Approval) -> bool;
}
