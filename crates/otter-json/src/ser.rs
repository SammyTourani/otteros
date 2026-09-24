//! Serialization: compact and pretty, both via a shared `core::fmt::Write`
//! writer so callers who already have a buffer (e.g. an outgoing HTTP body,
//! M9) don't pay for an intermediate allocation.

use alloc::format;
use alloc::string::String;
use core::fmt;

use crate::value::{Number, Value};

/// Serializes `value` as compact JSON (no extra whitespace).
pub fn to_string(value: &Value) -> String {
    let mut s = String::new();
    write_compact(value, &mut s).expect("writing to a String never fails");
    s
}

/// Serializes `value` as pretty JSON, two-space indented.
pub fn to_string_pretty(value: &Value) -> String {
    to_string_pretty_with_indent(value, "  ")
}

/// Like [`to_string_pretty`] with a caller-chosen indent unit (e.g. `"\t"`).
pub fn to_string_pretty_with_indent(value: &Value, indent: &str) -> String {
    let mut s = String::new();
    write_pretty(value, &mut s, indent).expect("writing to a String never fails");
    s
}

/// Writes `value` as compact JSON to any [`core::fmt::Write`] sink.
pub fn write_compact<W: fmt::Write>(value: &Value, w: &mut W) -> fmt::Result {
    match value {
        Value::Null => w.write_str("null"),
        Value::Bool(true) => w.write_str("true"),
        Value::Bool(false) => w.write_str("false"),
        Value::Number(n) => write_number(*n, w),
        Value::String(s) => write_escaped_str(s, w),
        Value::Array(items) => {
            w.write_char('[')?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    w.write_char(',')?;
                }
                write_compact(item, w)?;
            }
            w.write_char(']')
        }
        Value::Object(obj) => {
            w.write_char('{')?;
            for (i, (k, v)) in obj.iter().enumerate() {
                if i > 0 {
                    w.write_char(',')?;
                }
                write_escaped_str(k, w)?;
                w.write_char(':')?;
                write_compact(v, w)?;
            }
            w.write_char('}')
        }
    }
}

/// Writes `value` as pretty JSON to any [`core::fmt::Write`] sink.
pub fn write_pretty<W: fmt::Write>(value: &Value, w: &mut W, indent: &str) -> fmt::Result {
    write_pretty_inner(value, w, indent, 0)
}

fn write_pretty_inner<W: fmt::Write>(
    value: &Value,
    w: &mut W,
    indent: &str,
    level: usize,
) -> fmt::Result {
    match value {
        Value::Array(items) if !items.is_empty() => {
            w.write_char('[')?;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    w.write_char(',')?;
                }
                w.write_char('\n')?;
                write_indent(w, indent, level + 1)?;
                write_pretty_inner(item, w, indent, level + 1)?;
            }
            w.write_char('\n')?;
            write_indent(w, indent, level)?;
            w.write_char(']')
        }
        Value::Object(obj) if !obj.is_empty() => {
            w.write_char('{')?;
            for (i, (k, v)) in obj.iter().enumerate() {
                if i > 0 {
                    w.write_char(',')?;
                }
                w.write_char('\n')?;
                write_indent(w, indent, level + 1)?;
                write_escaped_str(k, w)?;
                w.write_str(": ")?;
                write_pretty_inner(v, w, indent, level + 1)?;
            }
            w.write_char('\n')?;
            write_indent(w, indent, level)?;
            w.write_char('}')
        }
        // null/bool/number/string, and empty arrays/objects, never wrap lines.
        _ => write_compact(value, w),
    }
}

fn write_indent<W: fmt::Write>(w: &mut W, indent: &str, level: usize) -> fmt::Result {
    for _ in 0..level {
        w.write_str(indent)?;
    }
    Ok(())
}

fn write_escaped_str<W: fmt::Write>(s: &str, w: &mut W) -> fmt::Result {
    w.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => w.write_str("\\\"")?,
            '\\' => w.write_str("\\\\")?,
            '\u{0008}' => w.write_str("\\b")?,
            '\u{000C}' => w.write_str("\\f")?,
            '\n' => w.write_str("\\n")?,
            '\r' => w.write_str("\\r")?,
            '\t' => w.write_str("\\t")?,
            c if (c as u32) < 0x20 => write!(w, "\\u{:04x}", c as u32)?,
            c => w.write_char(c)?,
        }
    }
    w.write_char('"')
}

fn write_number<W: fmt::Write>(n: Number, w: &mut W) -> fmt::Result {
    match n {
        Number::Int(i) => write!(w, "{i}"),
        Number::UInt(u) => write!(w, "{u}"),
        Number::Float(f) => write_f64(f, w),
    }
}

/// RFC 8259 has no literal for NaN/Infinity and no required notation for
/// ordinary numbers, so: non-finite floats become `null` (a still-valid-JSON
/// convention shared with `JSON.stringify`), and finite floats use fixed
/// notation for everyday magnitudes and scientific notation outside that
/// range (roughly Python's `repr` thresholds), always keeping a `.`
/// somewhere so a float round-trips as a float. Both branches format the
/// same correctly-rounded shortest decimal digits Rust's `f64::to_string`
/// produces (just arranged differently), so `parse(&write_f64(x)) == x` for
/// every finite `x`.
fn write_f64<W: fmt::Write>(f: f64, w: &mut W) -> fmt::Result {
    if !f.is_finite() {
        return w.write_str("null");
    }
    let sci = format!("{f:e}");
    let exp: i32 = sci[sci.find('e').expect("`{:e}` always contains 'e'") + 1..]
        .parse()
        .expect("`{:e}`'s exponent is always a plain decimal integer");
    if (-4..17).contains(&exp) {
        let fixed = format!("{f}");
        if fixed.contains('.') {
            w.write_str(&fixed)
        } else {
            write!(w, "{fixed}.0")
        }
    } else {
        w.write_str(&sci)
    }
}
