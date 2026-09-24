//! A strict, single-pass, recursive-descent RFC 8259 parser.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::{Error, ErrorKind};
use crate::value::{Number, Object, Value};

/// Maximum array/object nesting depth. Chosen generously above anything a
/// real config or tokenizer file needs (a few levels) while staying far
/// below the point where recursion could threaten the host stack; the
/// check happens *before* recursing into a nested array/object, so
/// pathological input (e.g. thousands of consecutive `[`) is rejected
/// immediately rather than causing deep recursion.
pub const MAX_DEPTH: usize = 128;

/// Parses `input` as a single JSON document. Trailing non-whitespace bytes
/// after the value are an error, matching RFC 8259 (a JSON text is exactly
/// one value).
pub fn parse(input: &[u8]) -> Result<Value, Error> {
    let mut p = Parser { input, pos: 0, depth: 0 };
    p.skip_ws();
    let value = p.parse_value()?;
    p.skip_ws();
    if p.pos != p.input.len() {
        return Err(p.err(ErrorKind::TrailingCharacters));
    }
    Ok(value)
}

/// Convenience wrapper around [`parse`] for a `&str`.
pub fn parse_str(input: &str) -> Result<Value, Error> {
    parse(input.as_bytes())
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn err(&self, kind: ErrorKind) -> Error {
        Error::new(self.pos, kind)
    }

    fn err_at(&self, offset: usize, kind: ErrorKind) -> Error {
        Error::new(offset, kind)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// Enters one more level of array/object nesting, failing if that would
    /// exceed [`MAX_DEPTH`]. Callers only need to undo this (`self.depth -=
    /// 1`) on their success path: any error return unwinds the whole parse
    /// via `?`, so a stale depth count on an error path is never observed.
    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH { Err(self.err(ErrorKind::DepthLimitExceeded)) } else { Ok(()) }
    }

    fn parse_value(&mut self) -> Result<Value, Error> {
        self.skip_ws();
        match self.peek() {
            None => Err(self.err(ErrorKind::UnexpectedEof)),
            Some(b'"') => self.parse_string().map(Value::String),
            Some(b'{') => self.parse_object().map(Value::Object),
            Some(b'[') => self.parse_array().map(Value::Array),
            Some(b't') => self.parse_lit(b"true", Value::Bool(true)),
            Some(b'f') => self.parse_lit(b"false", Value::Bool(false)),
            Some(b'n') => self.parse_lit(b"null", Value::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number().map(Value::Number),
            Some(b) => Err(self.err(ErrorKind::UnexpectedByte(b))),
        }
    }

    fn parse_lit(&mut self, lit: &'static [u8], value: Value) -> Result<Value, Error> {
        let start = self.pos;
        for &want in lit {
            if self.bump() != Some(want) {
                return Err(self.err_at(start, ErrorKind::InvalidLiteral));
            }
        }
        Ok(value)
    }

    fn parse_object(&mut self) -> Result<Object, Error> {
        self.pos += 1; // '{'
        self.enter()?;
        let mut obj = Object::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(obj);
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(match self.peek() {
                    None => self.err(ErrorKind::UnexpectedEof),
                    Some(_) => self.err(ErrorKind::ExpectedString),
                });
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(match self.peek() {
                    None => self.err(ErrorKind::UnexpectedEof),
                    Some(_) => self.err(ErrorKind::ExpectedColon),
                });
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.parse_value()?;
            obj.push(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b'}') {
                        return Err(self.err(ErrorKind::TrailingComma));
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(obj);
                }
                Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Vec<Value>, Error> {
        self.pos += 1; // '['
        self.enter()?;
        let mut arr = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.depth -= 1;
            return Ok(arr);
        }
        loop {
            self.skip_ws();
            arr.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_ws();
                    if self.peek() == Some(b']') {
                        return Err(self.err(ErrorKind::TrailingComma));
                    }
                }
                Some(b']') => {
                    self.pos += 1;
                    self.depth -= 1;
                    return Ok(arr);
                }
                Some(b) => return Err(self.err(ErrorKind::UnexpectedByte(b))),
                None => return Err(self.err(ErrorKind::UnexpectedEof)),
            }
        }
    }

    /// Reads exactly 4 hex digits starting at `self.pos`, advancing past
    /// them. `esc_pos` is only used to anchor the error location at the
    /// escape's leading `\`.
    fn parse_hex4(&mut self, esc_pos: usize) -> Result<u32, Error> {
        if self.pos + 4 > self.input.len() {
            return Err(self.err_at(esc_pos, ErrorKind::InvalidUnicodeEscape));
        }
        let mut cp: u32 = 0;
        for i in 0..4 {
            let d = match self.input[self.pos + i] {
                b @ b'0'..=b'9' => b - b'0',
                b @ b'a'..=b'f' => b - b'a' + 10,
                b @ b'A'..=b'F' => b - b'A' + 10,
                _ => return Err(self.err_at(esc_pos, ErrorKind::InvalidUnicodeEscape)),
            };
            cp = (cp << 4) | u32::from(d);
        }
        self.pos += 4;
        Ok(cp)
    }

    /// Handles a `\uXXXX` escape already consumed up through the `u`,
    /// pushing the resulting scalar value(s) onto `out`. A high surrogate
    /// followed by a matching low surrogate combines into one astral
    /// character (RFC 8259 §7's non-normative note on surrogate pairs); an
    /// unpaired surrogate (either half) is not representable in a Rust
    /// `String` (which requires valid UTF-8/scalar values), so it becomes
    /// U+FFFD REPLACEMENT CHARACTER -- a deliberate, documented,
    /// implementation-defined choice (JSONTestSuite's `i_` cases), never a
    /// panic or a rejection.
    fn parse_unicode_escape(&mut self, out: &mut String, esc_pos: usize) -> Result<(), Error> {
        let cp = self.parse_hex4(esc_pos)?;
        if (0xD800..=0xDBFF).contains(&cp) {
            let checkpoint = self.pos;
            if self.input.get(self.pos) == Some(&b'\\') && self.input.get(self.pos + 1) == Some(&b'u') {
                let low_esc_pos = self.pos;
                self.pos += 2; // consume the second '\u'
                let low = self.parse_hex4(low_esc_pos)?;
                if (0xDC00..=0xDFFF).contains(&low) {
                    let c = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                    // SAFETY-free: `c` is in 0x10000..=0x10FFFF by construction, a
                    // valid scalar value, so `unwrap_or` never takes the fallback.
                    out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                } else {
                    self.pos = checkpoint;
                    out.push('\u{FFFD}');
                }
            } else {
                out.push('\u{FFFD}');
            }
        } else if (0xDC00..=0xDFFF).contains(&cp) {
            out.push('\u{FFFD}');
        } else {
            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
        }
        Ok(())
    }

    fn parse_string(&mut self) -> Result<String, Error> {
        let start = self.pos;
        self.pos += 1; // opening '"'
        let mut out = String::new();
        loop {
            let run_start = self.pos;
            while let Some(&b) = self.input.get(self.pos) {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            if self.pos > run_start {
                match core::str::from_utf8(&self.input[run_start..self.pos]) {
                    Ok(s) => out.push_str(s),
                    Err(_) => return Err(self.err_at(run_start, ErrorKind::InvalidUtf8)),
                }
            }
            match self.peek() {
                None => return Err(self.err_at(start, ErrorKind::UnterminatedString)),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b) if b < 0x20 => return Err(self.err(ErrorKind::ControlCharacterInString)),
                Some(b'\\') => {
                    let esc_pos = self.pos;
                    self.pos += 1;
                    match self.bump() {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{0008}'),
                        Some(b'f') => out.push('\u{000C}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => self.parse_unicode_escape(&mut out, esc_pos)?,
                        Some(_) => return Err(self.err_at(esc_pos, ErrorKind::InvalidEscape)),
                        None => return Err(self.err_at(start, ErrorKind::UnterminatedString)),
                    }
                }
                Some(_) => unreachable!("run scan only stops at '\"', '\\\\' or a control byte"),
            }
        }
    }

    fn parse_number(&mut self) -> Result<Number, Error> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err_at(start, ErrorKind::InvalidNumber)),
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.pos += 1;
            match self.peek() {
                Some(b'0'..=b'9') => self.pos += 1,
                _ => return Err(self.err_at(start, ErrorKind::InvalidNumber)),
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            is_float = true;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            match self.peek() {
                Some(b'0'..=b'9') => self.pos += 1,
                _ => return Err(self.err_at(start, ErrorKind::InvalidNumber)),
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        // Every byte accepted above is ASCII (-, 0-9, ., e, E, +), so this is
        // always valid UTF-8.
        let token = core::str::from_utf8(&self.input[start..self.pos])
            .expect("number token bytes are all ASCII");
        if is_float {
            return token
                .parse::<f64>()
                .map(Number::Float)
                .map_err(|_| self.err_at(start, ErrorKind::InvalidNumber));
        }
        if let Ok(i) = token.parse::<i64>() {
            return Ok(Number::Int(i));
        }
        if let Ok(u) = token.parse::<u64>() {
            return Ok(Number::UInt(u));
        }
        token.parse::<f64>().map(Number::Float).map_err(|_| self.err_at(start, ErrorKind::InvalidNumber))
    }
}
