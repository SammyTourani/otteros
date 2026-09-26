use crate::os::OsError;

/// Check if a path is protected (case-insensitive, with component boundaries).
pub fn is_protected(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower == "/data/config" || lower.starts_with("/data/config/") ||
    lower == "/boot/modules" || lower.starts_with("/boot/modules/") ||
    lower == "/dev" || lower.starts_with("/dev/")
}

/// Check if a path is a writable location (case-insensitive, with component boundaries).
pub fn is_writable_location(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.starts_with("/data/") || lower.starts_with("/tmp/")
}

/// Check if data appears to be binary (contains null bytes).
pub fn is_binary(data: &[u8]) -> bool {
    data.contains(&0)
}

/// Check if a content type should be displayed as text.
pub fn should_show_as_text(content_type: &str) -> bool {
    let ct = content_type.to_lowercase();
    ct.starts_with("text/")
        || ct.starts_with("application/json")
        || ct.starts_with("application/xml")
        || ct.starts_with("application/javascript")
        || ct.contains("+json")
        || ct.contains("+xml")
}

/// Get a human-readable error message for an OsError.
pub fn error_message(err: &OsError) -> String {
    use alloc::string::ToString;
    match err {
        OsError::NotFound => "file or directory not found".to_string(),
        OsError::NotADirectory => "not a directory".to_string(),
        OsError::IsADirectory => "is a directory".to_string(),
        OsError::NoSpace => "no space available".to_string(),
        OsError::Timeout => "operation timed out".to_string(),
        OsError::Io(msg) => msg.clone(),
    }
}

use alloc::string::String;
