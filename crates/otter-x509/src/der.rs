//! A strict, minimal DER (Distinguished Encoding Rules, X.690) reader.
//!
//! X.509 certificates are DER, not the looser BER: definite-length encodings
//! only, every length in its minimal form, every INTEGER minimally encoded,
//! BIT STRINGs with their unused trailing bits zeroed, OBJECT IDENTIFIER
//! sub-identifiers minimally encoded. This module enforces all of that on the
//! way in rather than accepting a superset and hoping nothing downstream
//! relies on the stricter shape -- the same "reject anything that isn't
//! exactly the one true encoding" posture `otter-crypto`'s ECDSA signature
//! parsing takes (see its `ecc::EcdsaError::InvalidSignatureEncoding`).
//!
//! Every function here returns a `Result`; nothing panics on malformed
//! input, however deeply or repeatedly corrupted (see
//! `tests/robustness.rs`'s "flip every byte" fuzz test) -- every length is
//! checked against the remaining input before slicing, every arithmetic
//! step that could in principle overflow (adding to `pos`, decoding a
//! multi-byte length) is a checked operation that turns overflow into
//! [`DerError`] instead of a panic.
//!
//! Nesting depth is bounded: every [`Reader`] carries a `depth_budget` that
//! [`Reader::read_sequence`]/[`Reader::read_set`] (the only ways to obtain a
//! `Reader` over nested content) decrement, erroring instead of recursing
//! forever if a hostile input nests sequences past [`MAX_DEPTH`]. This
//! crate's own certificate grammar never nests anywhere near that deep; the
//! budget exists purely so a malformed/hostile certificate cannot make a
//! caller that walks nested content recurse without bound.

/// How many levels of SEQUENCE/SET nesting a single top-level DER value may
/// contain. X.509's own grammar (Certificate -> TBSCertificate -> Extensions
/// -> Extension -> extnValue's inner SEQUENCE -> ...) never nests past
/// single digits; this is a generous but finite ceiling so a crafted input
/// cannot force unbounded recursion.
pub const MAX_DEPTH: usize = 24;

/// A DER value failed to parse. Every variant is something a byte-corrupted
/// or hostile input can trigger; none of them come from a Rust panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerError {
    /// Ran out of input while reading a tag, a length, or content.
    UnexpectedEof,
    /// A high-tag-number form (tag byte's low 5 bits all set) was used. No
    /// tag this crate needs is numbered 31 or higher, so this form is
    /// always either hostile or a mistake.
    HighTagNumberUnsupported,
    /// The length used the indefinite form (BER only, never valid DER).
    IndefiniteLength,
    /// A long-form length was not encoded in the minimum number of octets,
    /// or used the long form for a value under 128 (which DER requires the
    /// short form for).
    NonMinimalLength,
    /// A long-form length's octet count did not fit this reader's internal
    /// accumulator (more than 8 octets: no certificate is anywhere near
    /// `2^64` bytes long).
    LengthOverflow,
    /// A TLV's declared length reaches past the end of the enclosing
    /// content.
    LengthOutOfBounds,
    /// The tag byte did not match what the caller expected.
    UnexpectedTag,
    /// Trailing bytes remained after the caller finished reading everything
    /// it expected from a value.
    TrailingData,
    /// An INTEGER was empty, or carried a non-minimal leading byte (a
    /// leading `0x00` not required to keep the value non-negative, or a
    /// leading `0xff` not required to keep it negative).
    InvalidInteger,
    /// An INTEGER value did not fit the caller's expected range (e.g. a
    /// negative value where only non-negative is valid, or a value that does
    /// not fit `u64` where the caller needs a small integer).
    IntegerOutOfRange,
    /// A BIT STRING's unused-bits octet was out of `0..=7`, its declared
    /// unused bits were not actually zero, or it declared unused bits on an
    /// empty bit string.
    InvalidBitString,
    /// An OBJECT IDENTIFIER was empty or had a sub-identifier encoded with a
    /// non-minimal (leading `0x80`) byte, or ended mid-sub-identifier.
    InvalidOid,
    /// A BOOLEAN's content was not exactly one octet, or was not the
    /// canonical `0x00`/`0xff`.
    InvalidBoolean,
    /// A time value (UTCTime/GeneralizedTime) did not match RFC 5280
    /// section 4.1.2.5's required exact format.
    InvalidTime,
    /// SEQUENCE/SET nesting went deeper than [`MAX_DEPTH`].
    DepthExceeded,
}

/// One decoded tag-length-value: `tag` is the raw first byte (class +
/// constructed bit + tag number, low-tag-number form only) and `content` is
/// exactly the value's bytes (already bounds-checked against the input).
#[derive(Clone, Copy, Debug)]
pub struct Tlv<'a> {
    /// The raw tag byte (class, constructed bit and tag number).
    pub tag: u8,
    /// The value's bytes, already bounds-checked against the input.
    pub content: &'a [u8],
}

/// Universal class tag numbers this crate reads, with the constructed bit
/// (`0x20`) already set where the type is always constructed.
pub mod tag {
    /// `BOOLEAN`.
    pub const BOOLEAN: u8 = 0x01;
    /// `INTEGER`.
    pub const INTEGER: u8 = 0x02;
    /// `BIT STRING`.
    pub const BIT_STRING: u8 = 0x03;
    /// `OCTET STRING`.
    pub const OCTET_STRING: u8 = 0x04;
    /// `NULL`.
    pub const NULL: u8 = 0x05;
    /// `OBJECT IDENTIFIER`.
    pub const OID: u8 = 0x06;
    /// `UTF8String`.
    pub const UTF8_STRING: u8 = 0x0c;
    /// `SEQUENCE` (constructed).
    pub const SEQUENCE: u8 = 0x30;
    /// `SET` (constructed).
    pub const SET: u8 = 0x31;
    /// `PrintableString`.
    pub const PRINTABLE_STRING: u8 = 0x13;
    /// `T61String` / `TeletexString`.
    pub const T61_STRING: u8 = 0x14;
    /// `IA5String`.
    pub const IA5_STRING: u8 = 0x16;
    /// `UTCTime`.
    pub const UTC_TIME: u8 = 0x17;
    /// `GeneralizedTime`.
    pub const GENERALIZED_TIME: u8 = 0x18;
    /// `UniversalString`.
    pub const UNIVERSAL_STRING: u8 = 0x1c;
    /// `BMPString`.
    pub const BMP_STRING: u8 = 0x1e;

    /// Builds a context-specific, constructed tag (`10 1 nnnnn`) for
    /// `[n] EXPLICIT`/implicit-constructed fields, e.g. `TBSCertificate`'s
    /// `[0] version` or `Certificate`'s `[3] extensions`.
    pub const fn context_constructed(n: u8) -> u8 {
        0xa0 | n
    }

    /// Builds a context-specific, primitive tag (`10 0 nnnnn`) for
    /// `[n] IMPLICIT` fields of a primitive underlying type, e.g.
    /// `GeneralName`'s `[2] dNSName` (IMPLICIT IA5String).
    pub const fn context_primitive(n: u8) -> u8 {
        0x80 | n
    }
}

/// A cursor over one DER-encoded value's content, reading successive TLVs
/// from the front. Cloning a `Reader` is cheap (a slice and two `usize`s)
/// and is how callers "peek": clone, try a read, discard the clone if it
/// was the wrong shape.
#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    depth_budget: usize,
}

impl<'a> Reader<'a> {
    /// A fresh top-level reader over `data`, with a full depth budget.
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, depth_budget: MAX_DEPTH }
    }

    fn with_budget(data: &'a [u8], depth_budget: usize) -> Reader<'a> {
        Reader { data, depth_budget }
    }

    /// Whether every byte of this reader's content has been consumed.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The bytes not yet consumed.
    pub fn remaining(&self) -> &'a [u8] {
        self.data
    }

    /// Reads one TLV and returns its *entire* encoding (tag, length and
    /// content octets together) -- what a signature over "this DER value"
    /// (e.g. a certificate's `TBSCertificate`) actually covers, as opposed
    /// to [`Reader::read_tlv`]'s `content`, which is only the value's
    /// payload.
    pub fn read_tlv_bytes(&mut self) -> Result<&'a [u8], DerError> {
        let start = self.data;
        self.read_tlv()?;
        let consumed = start.len() - self.data.len();
        Ok(&start[..consumed])
    }

    /// Reads one TLV, advancing past it. Enforces DER's length rules
    /// (definite, minimal) but does not interpret the tag.
    pub fn read_tlv(&mut self) -> Result<Tlv<'a>, DerError> {
        let (&tag, rest) = self.data.split_first().ok_or(DerError::UnexpectedEof)?;
        if tag & 0x1f == 0x1f {
            return Err(DerError::HighTagNumberUnsupported);
        }
        let (&len0, rest) = rest.split_first().ok_or(DerError::UnexpectedEof)?;
        let (len, rest) = if len0 & 0x80 == 0 {
            (len0 as usize, rest)
        } else {
            let n = (len0 & 0x7f) as usize;
            if n == 0 {
                return Err(DerError::IndefiniteLength);
            }
            if n > 8 {
                return Err(DerError::LengthOverflow);
            }
            if rest.len() < n {
                return Err(DerError::UnexpectedEof);
            }
            let (len_bytes, rest) = rest.split_at(n);
            if len_bytes[0] == 0 {
                // A leading zero octet in the long-form length is never
                // needed (it never disambiguates a length's magnitude the
                // way an INTEGER's sign octet can), so DER forbids it.
                return Err(DerError::NonMinimalLength);
            }
            let mut acc: u64 = 0;
            for &b in len_bytes {
                acc = acc.checked_shl(8).ok_or(DerError::LengthOverflow)?;
                acc |= u64::from(b);
            }
            let len: usize = acc.try_into().map_err(|_| DerError::LengthOverflow)?;
            if len < 128 {
                // Fits the short form; long form here is non-minimal.
                return Err(DerError::NonMinimalLength);
            }
            (len, rest)
        };
        if rest.len() < len {
            return Err(DerError::LengthOutOfBounds);
        }
        let (content, rest) = rest.split_at(len);
        self.data = rest;
        Ok(Tlv { tag, content })
    }

    /// Reads one TLV and checks its tag matches `expected`.
    pub fn expect_tag(&mut self, expected: u8) -> Result<&'a [u8], DerError> {
        let tlv = self.read_tlv()?;
        if tlv.tag != expected {
            return Err(DerError::UnexpectedTag);
        }
        Ok(tlv.content)
    }

    /// Peeks the next TLV's tag without consuming it (`None` at end of
    /// input, so optional trailing fields can be probed for cheaply).
    pub fn peek_tag(&self) -> Option<u8> {
        self.data.first().filter(|&&b| b & 0x1f != 0x1f).copied()
    }

    /// If the next TLV's tag is exactly `tag`, consumes and returns its
    /// content; otherwise leaves the reader untouched and returns `None`.
    /// Used for `OPTIONAL`/`DEFAULT` fields distinguished by tag (context
    /// tags, or a following field of a different universal tag).
    pub fn read_optional_tag(&mut self, tag: u8) -> Result<Option<&'a [u8]>, DerError> {
        if self.peek_tag() != Some(tag) {
            return Ok(None);
        }
        Ok(Some(self.expect_tag(tag)?))
    }

    fn child(&mut self, content: &'a [u8]) -> Result<Reader<'a>, DerError> {
        if self.depth_budget == 0 {
            return Err(DerError::DepthExceeded);
        }
        Ok(Reader::with_budget(content, self.depth_budget - 1))
    }

    /// Reads a `SEQUENCE`, returning a reader scoped to its content.
    pub fn read_sequence(&mut self) -> Result<Reader<'a>, DerError> {
        let content = self.expect_tag(tag::SEQUENCE)?;
        self.child(content)
    }

    /// Reads a `SET`, returning a reader scoped to its content.
    pub fn read_set(&mut self) -> Result<Reader<'a>, DerError> {
        let content = self.expect_tag(tag::SET)?;
        self.child(content)
    }

    /// Reads an explicitly-tagged `SEQUENCE`-shaped value if the next tag is
    /// `[n] EXPLICIT`, i.e. a constructed context tag wrapping exactly one
    /// inner TLV (used for e.g. `TBSCertificate`'s `[0] EXPLICIT Version`).
    pub fn read_explicit(&mut self, n: u8) -> Result<Option<Reader<'a>>, DerError> {
        let outer_tag = tag::context_constructed(n);
        match self.read_optional_tag(outer_tag)? {
            None => Ok(None),
            Some(content) => Ok(Some(self.child(content)?)),
        }
    }

    /// Reads an INTEGER's raw content bytes (big-endian, two's complement),
    /// checking DER's minimal-encoding rule. Content may be empty-checked by
    /// the caller if a non-negative value is required (use
    /// [`Reader::read_uint_bytes`] for that).
    pub fn read_integer_bytes(&mut self) -> Result<&'a [u8], DerError> {
        let content = self.expect_tag(tag::INTEGER)?;
        validate_integer(content)?;
        Ok(content)
    }

    /// Reads an INTEGER that must be non-negative, returning its big-endian
    /// bytes with any single leading `0x00` sign-forcing byte stripped (so
    /// callers get the plain magnitude, e.g. to feed `BigUint::from_be_bytes`).
    pub fn read_uint_bytes(&mut self) -> Result<&'a [u8], DerError> {
        let content = self.read_integer_bytes()?;
        if content[0] & 0x80 != 0 {
            return Err(DerError::IntegerOutOfRange); // negative
        }
        Ok(strip_leading_zero(content))
    }

    /// Reads a non-negative INTEGER small enough to fit `u64`.
    pub fn read_small_uint(&mut self) -> Result<u64, DerError> {
        decode_uint(self.read_integer_bytes()?)
    }

    /// Reads a BOOLEAN, requiring the canonical single-octet DER encoding
    /// (`0x00` false, `0xff` true).
    pub fn read_boolean(&mut self) -> Result<bool, DerError> {
        let content = self.expect_tag(tag::BOOLEAN)?;
        match content {
            [0x00] => Ok(false),
            [0xff] => Ok(true),
            _ => Err(DerError::InvalidBoolean),
        }
    }

    /// Reads a BOOLEAN with a `DEFAULT FALSE`: absent (next tag isn't
    /// BOOLEAN) means `false`.
    pub fn read_boolean_default_false(&mut self) -> Result<bool, DerError> {
        match self.read_optional_tag(tag::BOOLEAN)? {
            None => Ok(false),
            Some(content) => match content {
                [0x00] => Ok(false),
                [0xff] => Ok(true),
                _ => Err(DerError::InvalidBoolean),
            },
        }
    }

    /// Reads a BIT STRING, returning `(unused_bits, bytes)`. Validates the
    /// unused-bit count is `0..=7` (or `0` for an empty bit string) and that
    /// the unused low bits of the last byte are actually zero (DER's
    /// canonical-padding rule).
    pub fn read_bit_string(&mut self) -> Result<(u8, &'a [u8]), DerError> {
        let content = self.expect_tag(tag::BIT_STRING)?;
        let (&unused, bytes) = content.split_first().ok_or(DerError::InvalidBitString)?;
        if bytes.is_empty() {
            if unused != 0 {
                return Err(DerError::InvalidBitString);
            }
            return Ok((0, bytes));
        }
        if unused > 7 {
            return Err(DerError::InvalidBitString);
        }
        if unused > 0 {
            let mask = (1u8 << unused) - 1;
            let last = bytes[bytes.len() - 1];
            if last & mask != 0 {
                return Err(DerError::InvalidBitString);
            }
        }
        Ok((unused, bytes))
    }

    /// Reads a BIT STRING that must have zero unused bits (a whole number of
    /// bytes) -- the shape every BIT STRING in this crate's supported
    /// signature algorithms and key encodings uses.
    pub fn read_bit_string_bytes(&mut self) -> Result<&'a [u8], DerError> {
        let (unused, bytes) = self.read_bit_string()?;
        if unused != 0 {
            return Err(DerError::InvalidBitString);
        }
        Ok(bytes)
    }

    /// Reads an OCTET STRING's raw content.
    pub fn read_octet_string(&mut self) -> Result<&'a [u8], DerError> {
        self.expect_tag(tag::OCTET_STRING)
    }

    /// Reads a `NULL` (used for e.g. `rsaEncryption`'s absent
    /// `AlgorithmIdentifier` parameters).
    pub fn read_null(&mut self) -> Result<(), DerError> {
        let content = self.expect_tag(tag::NULL)?;
        if !content.is_empty() {
            return Err(DerError::UnexpectedTag);
        }
        Ok(())
    }

    /// Reads an OBJECT IDENTIFIER's raw content bytes (the concatenated
    /// base-128 sub-identifiers, arcs 1 and 2 combined per X.690), validating
    /// that every sub-identifier is minimally encoded (no sub-identifier
    /// starts with a superfluous `0x80` byte) and that content does not end
    /// mid-sub-identifier.
    pub fn read_oid(&mut self) -> Result<&'a [u8], DerError> {
        let content = self.expect_tag(tag::OID)?;
        if content.is_empty() {
            return Err(DerError::InvalidOid);
        }
        let mut at_start = true;
        for &b in content {
            if at_start && b == 0x80 {
                return Err(DerError::InvalidOid);
            }
            at_start = b & 0x80 == 0;
        }
        if !at_start {
            // Content ended with a byte whose continuation bit was still set.
            return Err(DerError::InvalidOid);
        }
        Ok(content)
    }
}

fn validate_integer(content: &[u8]) -> Result<(), DerError> {
    if content.is_empty() {
        return Err(DerError::InvalidInteger);
    }
    if content.len() > 1 {
        let (first, second) = (content[0], content[1]);
        // Minimal two's-complement: a leading 0x00 is only valid if the next
        // byte's high bit is set (otherwise the 0x00 was unnecessary); a
        // leading 0xff is only valid if the next byte's high bit is clear.
        if first == 0x00 && second & 0x80 == 0 {
            return Err(DerError::InvalidInteger);
        }
        if first == 0xff && second & 0x80 != 0 {
            return Err(DerError::InvalidInteger);
        }
    }
    Ok(())
}

fn strip_leading_zero(bytes: &[u8]) -> &[u8] {
    if bytes.len() > 1 && bytes[0] == 0x00 { &bytes[1..] } else { bytes }
}

/// Decodes an already-extracted INTEGER content (validated minimal, as
/// [`Reader::read_integer_bytes`] returns) as a non-negative value that fits
/// `u64`. Used both by [`Reader::read_small_uint`] and by callers decoding
/// an `IMPLICIT`-tagged integer whose content [`Reader::read_optional_tag`]
/// already extracted (so its own tag no longer reads as `INTEGER`, but the
/// content encoding rules are identical for a primitive type).
pub fn decode_uint(content: &[u8]) -> Result<u64, DerError> {
    validate_integer(content)?;
    if content[0] & 0x80 != 0 {
        return Err(DerError::IntegerOutOfRange);
    }
    let bytes = strip_leading_zero(content);
    if bytes.len() > 8 {
        return Err(DerError::IntegerOutOfRange);
    }
    let mut buf = [0u8; 8];
    buf[8 - bytes.len()..].copy_from_slice(bytes);
    Ok(u64::from_be_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_simple_sequence_of_integers() {
        // SEQUENCE { INTEGER 1, INTEGER 2 }
        let der = [0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x02];
        let mut r = Reader::new(&der);
        let mut seq = r.read_sequence().unwrap();
        assert_eq!(seq.read_small_uint().unwrap(), 1);
        assert_eq!(seq.read_small_uint().unwrap(), 2);
        assert!(seq.is_empty());
        assert!(r.is_empty());
    }

    #[test]
    fn read_tlv_bytes_includes_tag_and_length() {
        // SEQUENCE { INTEGER 1, INTEGER 2 } followed by a trailing byte.
        let der = [0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x02, 0xff];
        let mut r = Reader::new(&der);
        let whole = r.read_tlv_bytes().unwrap();
        assert_eq!(whole, &der[..8]); // tag + length + content, not just content
        assert_eq!(r.remaining(), &der[8..]); // the trailing 0xff is untouched
    }

    #[test]
    fn rejects_trailing_data() {
        let der = [0x02, 0x01, 0x01, 0xff];
        let mut r = Reader::new(&der);
        let _ = r.read_integer_bytes().unwrap();
        assert!(!r.is_empty());
    }

    #[test]
    fn rejects_indefinite_length() {
        let der = [0x30, 0x80, 0x00, 0x00];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_sequence().err(), Some(DerError::IndefiniteLength));
    }

    #[test]
    fn rejects_non_minimal_long_form_length() {
        // Length 5 encoded in long form (0x81 0x05) instead of short form.
        let der = [0x04, 0x81, 0x05, 1, 2, 3, 4, 5];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_octet_string().err(), Some(DerError::NonMinimalLength));
    }

    #[test]
    fn rejects_non_minimal_length_leading_zero() {
        let der = [0x04, 0x82, 0x00, 0x05, 1, 2, 3, 4, 5];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_octet_string().err(), Some(DerError::NonMinimalLength));
    }

    #[test]
    fn accepts_long_form_length_at_128() {
        let mut der = vec![0x04, 0x81, 128];
        der.extend([0u8; 128]);
        let mut r = Reader::new(&der);
        assert_eq!(r.read_octet_string().unwrap().len(), 128);
    }

    #[test]
    fn rejects_length_past_end_of_input() {
        let der = [0x04, 0x05, 1, 2];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_octet_string().err(), Some(DerError::LengthOutOfBounds));
    }

    #[test]
    fn rejects_high_tag_number_form() {
        let der = [0x1f, 0x01, 0x00];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_tlv().err(), Some(DerError::HighTagNumberUnsupported));
    }

    #[test]
    fn integer_rejects_empty_content() {
        let der = [0x02, 0x00];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_integer_bytes().err(), Some(DerError::InvalidInteger));
    }

    #[test]
    fn integer_rejects_non_minimal_leading_zero() {
        let der = [0x02, 0x02, 0x00, 0x01]; // could have been just 0x01
        let mut r = Reader::new(&der);
        assert_eq!(r.read_integer_bytes().err(), Some(DerError::InvalidInteger));
    }

    #[test]
    fn integer_accepts_leading_zero_when_needed_for_sign() {
        let der = [0x02, 0x02, 0x00, 0x80]; // 0x80 alone would be negative
        let mut r = Reader::new(&der);
        assert_eq!(r.read_integer_bytes().unwrap(), [0x00, 0x80]);
    }

    #[test]
    fn integer_rejects_non_minimal_leading_ff() {
        let der = [0x02, 0x02, 0xff, 0x80]; // could have been just 0x80
        let mut r = Reader::new(&der);
        assert_eq!(r.read_integer_bytes().err(), Some(DerError::InvalidInteger));
    }

    #[test]
    fn uint_bytes_rejects_negative() {
        let der = [0x02, 0x01, 0x80];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_uint_bytes().err(), Some(DerError::IntegerOutOfRange));
    }

    #[test]
    fn small_uint_round_trips() {
        let der = [0x02, 0x02, 0x01, 0x00];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_small_uint().unwrap(), 256);
    }

    #[test]
    fn boolean_requires_canonical_encoding() {
        let der = [0x01, 0x01, 0x01]; // true, but not canonical 0xff
        let mut r = Reader::new(&der);
        assert_eq!(r.read_boolean().err(), Some(DerError::InvalidBoolean));
    }

    #[test]
    fn boolean_default_false_when_absent() {
        let der = [0x02, 0x01, 0x05]; // an INTEGER, not a BOOLEAN
        let mut r = Reader::new(&der);
        assert!(!r.read_boolean_default_false().unwrap());
        // Nothing consumed: the INTEGER is still there.
        assert_eq!(r.read_small_uint().unwrap(), 5);
    }

    #[test]
    fn bit_string_rejects_dirty_padding_bits() {
        let der = [0x03, 0x02, 0x01, 0b1000_0001]; // 1 unused bit, but it's 1 not 0
        let mut r = Reader::new(&der);
        assert_eq!(r.read_bit_string().err(), Some(DerError::InvalidBitString));
    }

    #[test]
    fn bit_string_accepts_clean_padding() {
        let der = [0x03, 0x02, 0x01, 0b1000_0000];
        let mut r = Reader::new(&der);
        let (unused, bytes) = r.read_bit_string().unwrap();
        assert_eq!(unused, 1);
        assert_eq!(bytes, [0b1000_0000]);
    }

    #[test]
    fn bit_string_empty_requires_zero_unused() {
        let der = [0x03, 0x01, 0x00];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_bit_string().unwrap(), (0, &[][..]));
    }

    #[test]
    fn oid_rejects_non_minimal_subidentifier() {
        // 1.2 (0x2a) followed by a sub-identifier with a superfluous 0x80 lead byte.
        let der = [0x06, 0x03, 0x2a, 0x80, 0x01];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_oid().err(), Some(DerError::InvalidOid));
    }

    #[test]
    fn oid_rejects_truncated_subidentifier() {
        let der = [0x06, 0x02, 0x2a, 0x81]; // continuation bit set, then nothing
        let mut r = Reader::new(&der);
        assert_eq!(r.read_oid().err(), Some(DerError::InvalidOid));
    }

    #[test]
    fn oid_accepts_rsa_encryption() {
        // 1.2.840.113549.1.1.1
        let der = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
        let mut r = Reader::new(&der);
        assert_eq!(r.read_oid().unwrap(), &der[2..]);
    }

    /// Builds `levels` empty SEQUENCEs nested inside one another
    /// (innermost-first construction: `30 00`, then `30 02 30 00`, ...).
    fn nested_sequences(levels: usize) -> Vec<u8> {
        let mut encoded: Vec<u8> = vec![];
        for _ in 0..levels {
            let mut next = vec![0x30, encoded.len() as u8];
            next.extend_from_slice(&encoded);
            encoded = next;
        }
        encoded
    }

    #[test]
    fn depth_budget_is_enforced() {
        let der = nested_sequences(40);
        let mut reader = Reader::new(&der);
        let mut successes = 0;
        loop {
            match reader.read_sequence() {
                Ok(inner) => {
                    successes += 1;
                    reader = inner;
                }
                Err(e) => {
                    assert_eq!(e, DerError::DepthExceeded);
                    break;
                }
            }
            if successes > MAX_DEPTH + 5 {
                panic!("depth budget was never enforced after {successes} nested SEQUENCEs");
            }
        }
        assert_eq!(successes, MAX_DEPTH, "expected exactly MAX_DEPTH successful nestings before DepthExceeded");
    }

    #[test]
    fn shallow_nesting_within_budget_succeeds() {
        let der = nested_sequences(MAX_DEPTH);
        let mut reader = Reader::new(&der);
        for _ in 0..MAX_DEPTH {
            reader = reader.read_sequence().expect("within budget");
        }
        assert!(reader.is_empty());
    }
}
