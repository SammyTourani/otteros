//! RFC 8259 strictness and byte-offset error reporting (brief M7-T1):
//! no trailing commas, no comments, and every rejection points at (or very
//! near) the offending byte.

use otter_json::{ErrorKind, parse_str};

fn err(src: &str) -> otter_json::Error {
    parse_str(src).expect_err(&format!("expected {src:?} to be rejected"))
}

#[test]
fn trailing_comma_in_array_is_rejected() {
    let e = err("[1,2,]");
    assert_eq!(e.kind, ErrorKind::TrailingComma);
    assert_eq!(e.offset, 5, "offset of the ']' that follows the illegal comma");
}

#[test]
fn trailing_comma_in_object_is_rejected() {
    let e = err(r#"{"a":1,}"#);
    assert_eq!(e.kind, ErrorKind::TrailingComma);
}

#[test]
fn leading_comma_is_rejected() {
    assert!(parse_str("[,1]").is_err());
}

#[test]
fn line_comment_is_rejected() {
    assert!(parse_str("[1, 2] // comment").is_err());
    assert!(parse_str("// comment\n[1]").is_err());
}

#[test]
fn block_comment_is_rejected() {
    assert!(parse_str("[1 /* comment */, 2]").is_err());
}

#[test]
fn single_quoted_strings_are_rejected() {
    assert!(parse_str("['a']").is_err());
}

#[test]
fn unquoted_object_keys_are_rejected() {
    assert!(parse_str("{a:1}").is_err());
}

#[test]
fn bare_word_literals_are_case_sensitive() {
    assert!(parse_str("True").is_err());
    assert!(parse_str("NULL").is_err());
    assert!(parse_str("true").is_ok());
}

#[test]
fn leading_zero_is_rejected() {
    assert!(parse_str("01").is_err());
    assert!(parse_str("[01]").is_err());
    assert!(parse_str("-01").is_err());
}

#[test]
fn number_grammar_edge_cases_are_rejected() {
    assert!(parse_str(".5").is_err());
    assert!(parse_str("5.").is_err());
    assert!(parse_str("5.e1").is_err());
    assert!(parse_str("+5").is_err());
    assert!(parse_str("5e").is_err());
    assert!(parse_str("5e+").is_err());
    assert!(parse_str("-").is_err());
    assert!(parse_str("NaN").is_err());
    assert!(parse_str("Infinity").is_err());
}

#[test]
fn unescaped_control_character_in_string_is_rejected() {
    let e = err("\"a\tb\"");
    assert_eq!(e.kind, ErrorKind::ControlCharacterInString);
}

#[test]
fn unterminated_string_is_rejected() {
    let e = err("\"abc");
    assert_eq!(e.kind, ErrorKind::UnterminatedString);
    assert_eq!(e.offset, 0, "anchored at the opening quote");
}

#[test]
fn invalid_escape_character_is_rejected() {
    let e = err(r#""\x41""#);
    assert_eq!(e.kind, ErrorKind::InvalidEscape);
    assert_eq!(e.offset, 1, "anchored at the backslash");
}

#[test]
fn truncated_unicode_escape_is_rejected() {
    assert_eq!(err(r#""\u12""#).kind, ErrorKind::InvalidUnicodeEscape);
    assert_eq!(err(r#""\u12zz""#).kind, ErrorKind::InvalidUnicodeEscape);
}

#[test]
fn invalid_utf8_byte_in_string_is_rejected() {
    // 0xFF can never appear in valid UTF-8, so this raw byte string (built by
    // hand since it isn't representable as a Rust `&str` literal) must be
    // rejected rather than panicking while building the `String`.
    let raw: &[u8] = &[b'"', b'a', 0xFF, b'"'];
    let e = otter_json::parse(raw).expect_err("invalid UTF-8 must be rejected");
    assert_eq!(e.kind, ErrorKind::InvalidUtf8);
}

#[test]
fn missing_colon_and_missing_comma_are_rejected() {
    assert_eq!(err(r#"{"a" 1}"#).kind, ErrorKind::ExpectedColon);
    assert!(parse_str(r#"{"a":1 "b":2}"#).is_err());
    assert!(parse_str("[1 2]").is_err());
}

#[test]
fn object_key_must_be_a_string() {
    assert_eq!(err("{1:2}").kind, ErrorKind::ExpectedString);
}

#[test]
fn trailing_characters_after_the_top_level_value_are_rejected() {
    let e = err("1 2");
    assert_eq!(e.kind, ErrorKind::TrailingCharacters);
    assert_eq!(e.offset, 2);
}

#[test]
fn empty_input_is_rejected() {
    assert_eq!(err("").kind, ErrorKind::UnexpectedEof);
    assert_eq!(err("   ").kind, ErrorKind::UnexpectedEof);
}

#[test]
fn only_rfc_8259_whitespace_is_skipped_between_tokens() {
    assert!(parse_str("[1,\t\n\r 2]").is_ok());
    // Form feed and vertical tab are not JSON whitespace.
    assert!(parse_str("[1,\u{000C}2]").is_err());
    assert!(parse_str("[1,\u{000B}2]").is_err());
}

#[test]
fn error_display_includes_the_byte_offset() {
    let e = err("[1,]");
    let msg = format!("{e}");
    assert!(msg.contains("byte offset 3"), "message was: {msg}");
}
