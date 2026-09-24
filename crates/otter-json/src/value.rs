//! The JSON DOM: [`Value`], [`Number`] and [`Object`].

use alloc::string::String;
use alloc::vec::Vec;

/// A JSON number, kept as whichever Rust type represents it exactly.
///
/// RFC 8259 numbers have no inherent type distinction, but a Rust API needs
/// one. Integer literals (no `.` and no exponent) that fit in [`i64`] are
/// kept exactly as [`Number::Int`]; positive integer literals too large for
/// `i64` but that fit in [`u64`] are kept exactly as [`Number::UInt`]. Every
/// other numeric literal -- one with a fractional part or exponent, or an
/// integer literal too large for either 64-bit integer type -- becomes
/// [`Number::Float`]. This matches the common convention used by most JSON
/// libraries without arbitrary-precision support: exact for the integers
/// that matter (array lengths, token ids, vocab sizes, ...), lossy only for
/// magnitudes no real config or tokenizer file uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// An integer literal that fits in `i64` (includes all of `0`..=`i64::MAX`
    /// and negative values down to `i64::MIN`).
    Int(i64),
    /// A positive integer literal larger than `i64::MAX` but that fits in `u64`.
    UInt(u64),
    /// Any number with a `.` or exponent, or an integer literal too large for
    /// `i64`/`u64` in either direction.
    Float(f64),
}

impl Number {
    /// Returns the value as `i64` if it is exactly representable.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Number::Int(n) => Some(n),
            Number::UInt(n) => i64::try_from(n).ok(),
            // `core` has no `f64::fract`/`trunc` (they need libm, std-only --
            // see otter-gfx's README on `floor`/`ceil`/`round`/`sqrt`), so
            // exactness is checked by truncating via `as` and casting back.
            Number::Float(f) => {
                if f >= i64::MIN as f64 && f <= i64::MAX as f64 && (f as i64) as f64 == f {
                    Some(f as i64)
                } else {
                    None
                }
            }
        }
    }

    /// Returns the value as `u64` if it is exactly representable.
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Number::Int(n) => u64::try_from(n).ok(),
            Number::UInt(n) => Some(n),
            Number::Float(f) => {
                if f >= 0.0 && f <= u64::MAX as f64 && (f as u64) as f64 == f {
                    Some(f as u64)
                } else {
                    None
                }
            }
        }
    }

    /// Returns the value as `f64`. Always succeeds (possibly lossily for
    /// very large `u64`/`i64` magnitudes, exactly like an explicit `as f64`
    /// cast).
    pub fn as_f64(&self) -> f64 {
        match *self {
            Number::Int(n) => n as f64,
            Number::UInt(n) => n as f64,
            Number::Float(f) => f,
        }
    }

    /// True if this number was parsed from a literal with no `.` or exponent
    /// (regardless of whether it ended up as [`Number::Int`], [`Number::UInt`]
    /// or, for literals too big for either, [`Number::Float`]).
    pub fn is_integer_literal(&self) -> bool {
        !matches!(self, Number::Float(_))
    }
}

impl From<i64> for Number {
    fn from(v: i64) -> Self {
        Number::Int(v)
    }
}

impl From<u64> for Number {
    fn from(v: u64) -> Self {
        match i64::try_from(v) {
            Ok(n) => Number::Int(n),
            Err(_) => Number::UInt(v),
        }
    }
}

impl From<f64> for Number {
    fn from(v: f64) -> Self {
        Number::Float(v)
    }
}

/// A JSON object: an ordered list of key/value pairs.
///
/// RFC 8259 does not mandate an order or forbid duplicate keys. We preserve
/// source (or insertion) order -- needed for things like `tokenizer.json`'s
/// vocab map, where the order concerns nothing semantically but round-tripping
/// predictably is still nice -- and we preserve duplicates exactly as parsed
/// rather than silently dropping data. [`Object::get`] resolves duplicates by
/// returning the *last* matching entry, the convention most JSON consumers
/// (browsers included) use for "later keys win".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object {
    entries: Vec<(String, Value)>,
}

impl Object {
    /// An empty object.
    pub fn new() -> Self {
        Object { entries: Vec::new() }
    }

    /// Number of key/value pairs, counting duplicate keys separately.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the object has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends a key/value pair without checking for an existing key
    /// (used by the parser, which must preserve duplicates exactly as read).
    pub fn push(&mut self, key: String, value: Value) {
        self.entries.push((key, value));
    }

    /// Inserts a key/value pair. If the key already exists, its *last*
    /// occurrence's value is replaced in place and the old value returned;
    /// otherwise the pair is appended.
    pub fn insert(&mut self, key: String, value: Value) -> Option<Value> {
        if let Some(slot) = self.entries.iter_mut().rev().find(|(k, _)| *k == key) {
            Some(core::mem::replace(&mut slot.1, value))
        } else {
            self.entries.push((key, value));
            None
        }
    }

    /// The value of the last entry with this key, if any.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// True if any entry has this key.
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// Removes and returns the *first* entry with this key, if any (leaving
    /// any duplicates in place, matching `get`'s "last wins" reading pairing
    /// with "first out" would be surprising; in practice callers only ever
    /// call this on objects they know have no duplicate keys).
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(idx).1)
    }

    /// Iterates entries in source/insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl FromIterator<(String, Value)> for Object {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        Object { entries: iter.into_iter().collect() }
    }
}

impl<'a> IntoIterator for &'a Object {
    type Item = (&'a str, &'a Value);
    type IntoIter = alloc::boxed::Box<dyn Iterator<Item = (&'a str, &'a Value)> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        alloc::boxed::Box::new(self.iter())
    }
}

/// A JSON value: the DOM produced by [`crate::parse`] and consumed by
/// [`crate::to_string`]/[`crate::to_string_pretty`].
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `null`
    Null,
    /// `true` or `false`
    Bool(bool),
    /// Any JSON number; see [`Number`].
    Number(Number),
    /// A JSON string, already unescaped.
    String(String),
    /// A JSON array, in source order.
    Array(Vec<Value>),
    /// A JSON object; see [`Object`].
    Object(Object),
}

impl Value {
    /// Convenience: build a string value from anything `Into<String>`.
    pub fn string(s: impl Into<String>) -> Value {
        Value::String(s.into())
    }

    /// True for [`Value::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The bool, if this is [`Value::Bool`].
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The string slice, if this is [`Value::String`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// The [`Number`], if this is [`Value::Number`].
    pub fn as_number(&self) -> Option<Number> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Shorthand for `as_number().and_then(|n| n.as_i64())`.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(|n| n.as_i64())
    }

    /// Shorthand for `as_number().and_then(|n| n.as_u64())`.
    pub fn as_u64(&self) -> Option<u64> {
        self.as_number().and_then(|n| n.as_u64())
    }

    /// Shorthand for `as_number().map(|n| n.as_f64())`.
    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().map(|n| n.as_f64())
    }

    /// The element slice, if this is [`Value::Array`].
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a.as_slice()),
            _ => None,
        }
    }

    /// The [`Object`], if this is [`Value::Object`].
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    /// If this is an object, the value for `key` (see [`Object::get`]).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|o| o.get(key))
    }

    /// If this is an array, the element at `index`.
    pub fn index(&self, index: usize) -> Option<&Value> {
        self.as_array().and_then(|a| a.get(index))
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Number(Number::Int(v))
    }
}

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::Number(v.into())
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Number(Number::Float(v))
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(String::from(v))
    }
}

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Array(v)
    }
}

impl From<Object> for Value {
    fn from(v: Object) -> Self {
        Value::Object(v)
    }
}
