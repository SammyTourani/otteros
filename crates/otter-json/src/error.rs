//! Parse errors with a byte offset into the original input.

use core::fmt;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The input ended where more was expected.
    UnexpectedEof,
    /// A byte that cannot start or continue the current production.
    UnexpectedByte(u8),
    /// `t`/`f`/`n` did not spell `true`/`false`/`null`.
    InvalidLiteral,
    /// A `-`, digit run, `.` or exponent did not match the RFC 8259 number
    /// grammar (leading zeros, a bare `.` with no digits, a bare exponent
    /// with no digits, a trailing `-`/`+`, ...).
    InvalidNumber,
    /// A `\u` escape was not followed by exactly four hex digits.
    InvalidUnicodeEscape,
    /// An escape character after `\` other than `" \ / b f n r t u`.
    InvalidEscape,
    /// A string's bytes were not valid UTF-8.
    InvalidUtf8,
    /// A string's closing `"` was never found.
    UnterminatedString,
    /// A raw (unescaped) control byte (`0x00..=0x1F`) inside a string.
    ControlCharacterInString,
    /// A `,` immediately followed by `]` or `}` (no trailing commas in RFC 8259).
    TrailingComma,
    /// Extra non-whitespace input after the single top-level value.
    TrailingCharacters,
    /// Array/object nesting exceeded [`crate::MAX_DEPTH`].
    DepthLimitExceeded,
    /// An object entry's `:` was missing.
    ExpectedColon,
    /// An object key must be a string.
    ExpectedString,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::UnexpectedEof => write!(f, "unexpected end of input"),
            ErrorKind::UnexpectedByte(b) => {
                if b.is_ascii_graphic() || *b == b' ' {
                    write!(f, "unexpected byte {:#04x} ({:?})", b, *b as char)
                } else {
                    write!(f, "unexpected byte {:#04x}", b)
                }
            }
            ErrorKind::InvalidLiteral => write!(f, "invalid literal (expected true/false/null)"),
            ErrorKind::InvalidNumber => write!(f, "invalid number literal"),
            ErrorKind::InvalidUnicodeEscape => write!(f, "invalid \\u escape (need 4 hex digits)"),
            ErrorKind::InvalidEscape => write!(f, "invalid escape character"),
            ErrorKind::InvalidUtf8 => write!(f, "invalid UTF-8 in string"),
            ErrorKind::UnterminatedString => write!(f, "unterminated string"),
            ErrorKind::ControlCharacterInString => {
                write!(f, "unescaped control character in string")
            }
            ErrorKind::TrailingComma => write!(f, "trailing comma"),
            ErrorKind::TrailingCharacters => write!(f, "trailing characters after value"),
            ErrorKind::DepthLimitExceeded => write!(f, "nesting depth limit exceeded"),
            ErrorKind::ExpectedColon => write!(f, "expected ':' after object key"),
            ErrorKind::ExpectedString => write!(f, "expected a string (object key)"),
        }
    }
}

/// A parse error: what went wrong, and the byte offset into the input where
/// it was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    /// Byte offset into the original input.
    pub offset: usize,
    /// What went wrong.
    pub kind: ErrorKind,
}

impl Error {
    pub(crate) fn new(offset: usize, kind: ErrorKind) -> Self {
        Error { offset, kind }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte offset {}", self.kind, self.offset)
    }
}
