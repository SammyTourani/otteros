//! GUID parsing and formatting (8-4-4-4-12 hex digits in on-disk byte order).

use core::fmt;

/// A 128-bit GUID in on-disk byte order (little-endian for time fields, big-endian for the rest).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// Parses a GUID from a canonical text format: 8-4-4-4-12 hex digits (case-insensitive).
    /// Returns None if the format is invalid.
    pub fn parse(text: &str) -> Option<Guid> {
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
            return None;
        }

        let mut guid = [0u8; 16];

        // Parse 8-digit hex (first u32, little-endian)
        let v0 = parse_hex_u32(&bytes[0..8])?;
        guid[0..4].copy_from_slice(&v0.to_le_bytes());

        // Parse 4-digit hex (first u16, little-endian)
        let v1 = parse_hex_u16(&bytes[9..13])?;
        guid[4..6].copy_from_slice(&v1.to_le_bytes());

        // Parse 4-digit hex (second u16, little-endian)
        let v2 = parse_hex_u16(&bytes[14..18])?;
        guid[6..8].copy_from_slice(&v2.to_le_bytes());

        // Parse 4-digit hex (first u16, big-endian)
        let v3 = parse_hex_u16(&bytes[19..23])?;
        guid[8..10].copy_from_slice(&v3.to_be_bytes());

        // Parse 12-digit hex (last 6 bytes, big-endian)
        let v4 = parse_hex_u64(&bytes[24..36])?;
        guid[10..16].copy_from_slice(&v4.to_be_bytes()[2..]);

        Some(Guid(guid))
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = &self.0;
        // Reconstruct the three components from on-disk format to text format.
        let v0 = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let v1 = u16::from_le_bytes([bytes[4], bytes[5]]);
        let v2 = u16::from_le_bytes([bytes[6], bytes[7]]);
        let v3 = u16::from_be_bytes([bytes[8], bytes[9]]);
        let v4 = u64::from_be_bytes([0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]]);

        write!(f, "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}", v0, v1, v2, v3, v4)
    }
}

/// Parses a hex string into a u32.
fn parse_hex_u32(bytes: &[u8]) -> Option<u32> {
    let mut value = 0u32;
    for &b in bytes {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'A'..=b'F' => b - b'A' + 10,
            b'a'..=b'f' => b - b'a' + 10,
            _ => return None,
        };
        value = value.wrapping_mul(16).wrapping_add(digit as u32);
    }
    Some(value)
}

/// Parses a hex string into a u16.
fn parse_hex_u16(bytes: &[u8]) -> Option<u16> {
    let mut value = 0u16;
    for &b in bytes {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'A'..=b'F' => b - b'A' + 10,
            b'a'..=b'f' => b - b'a' + 10,
            _ => return None,
        };
        value = value.wrapping_mul(16).wrapping_add(digit as u16);
    }
    Some(value)
}

/// Parses a hex string into a u64.
fn parse_hex_u64(bytes: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for &b in bytes {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'A'..=b'F' => b - b'A' + 10,
            b'a'..=b'f' => b - b'a' + 10,
            _ => return None,
        };
        value = value.wrapping_mul(16).wrapping_add(digit as u64);
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn test_guid_parse_display() {
        let text = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
        let guid = Guid::parse(text).expect("parse failed");
        let formatted = guid.to_string();
        assert_eq!(formatted, text);
    }

    #[test]
    fn test_guid_case_insensitive() {
        let upper = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
        let lower = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
        assert_eq!(Guid::parse(upper), Guid::parse(lower));
    }

    #[test]
    fn test_guid_parse_invalid() {
        assert_eq!(Guid::parse(""), None);
        assert_eq!(Guid::parse("C12A7328F81F11D2BA4B00A0C93EC93B"), None);
        assert_eq!(Guid::parse("C12A7328-F81F-11D2-BA4B-00A0C93EC93"), None);
        assert_eq!(Guid::parse("G12A7328-F81F-11D2-BA4B-00A0C93EC93B"), None);
    }
}
