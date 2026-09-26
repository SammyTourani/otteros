use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    EmptyPath,
    NotAbsolute,
    InvalidComponent,
    TooLong,
}

/// Normalize an absolute path: resolve `.` and `..`, remove repeated slashes,
/// validate component names, ensure total length <= 1024 bytes.
///
/// FAT compares names case-insensitively, so policies using this function
/// match case-insensitively without explicit casing.
pub fn normalize(path: &str) -> Result<String, PathError> {
    if path.is_empty() {
        return Err(PathError::EmptyPath);
    }

    if !path.starts_with('/') {
        return Err(PathError::NotAbsolute);
    }

    // Split by '/' and filter empty parts (handles repeated slashes).
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();

    let mut normalized: Vec<String> = Vec::new();

    for part in parts {
        if part == "." {
            // Current directory reference; skip.
            continue;
        } else if part == ".." {
            // Parent directory reference; pop if we have anything.
            normalized.pop();
        } else {
            // Validate the component.
            if !is_valid_component(part) {
                return Err(PathError::InvalidComponent);
            }
            normalized.push(part.to_string());
        }
    }

    // Build the result path.
    let mut result = String::from("/");
    for (i, part) in normalized.iter().enumerate() {
        if i > 0 {
            result.push('/');
        }
        result.push_str(part);
    }

    // Check total length (max 1024 bytes).
    if result.len() > 1024 {
        return Err(PathError::TooLong);
    }

    Ok(result)
}

/// Check if a path component is valid:
/// - 1-255 bytes
/// - No forbidden chars: \ : * ? " < > | or control characters
/// - Not ending in . or space
fn is_valid_component(comp: &str) -> bool {
    let bytes = comp.as_bytes();

    // Must be 1-255 bytes.
    if bytes.is_empty() || bytes.len() > 255 {
        return false;
    }

    // Cannot end with . or space.
    if comp.ends_with('.') || comp.ends_with(' ') {
        return false;
    }

    // Check for forbidden characters and control characters.
    for &b in bytes {
        match b {
            b'\\' | b':' | b'*' | b'?' | b'"' | b'<' | b'>' | b'|' => return false,
            0..=31 | 127 => return false, // Control characters.
            _ => {}
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_normalization() {
        assert_eq!(normalize("/").unwrap(), "/");
        assert_eq!(normalize("/data").unwrap(), "/data");
        assert_eq!(normalize("/data/").unwrap(), "/data");
        assert_eq!(normalize("//data///notes//").unwrap(), "/data/notes");
        assert_eq!(normalize("/data/./notes/../notes/today.txt").unwrap(), "/data/notes/today.txt");
        assert_eq!(normalize("/../../data").unwrap(), "/data");
        assert_eq!(normalize("/data/..").unwrap(), "/");
    }

    #[test]
    fn test_invalid_paths() {
        assert!(normalize("").is_err());
        assert!(normalize("data/notes").is_err());
        assert!(normalize("/data/a:b").is_err());
        assert!(normalize("/data/a*b").is_err());
        assert!(normalize("/data/dot.").is_err());
        assert!(normalize("/data/space ").is_err());
    }
}
