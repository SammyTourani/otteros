//! html5lib tokenizer conformance, the acceptance oracle for brief M5-T0b, written by the
//! orchestrator: the crate must pass this file unchanged (the older html5lib_tokenizer.rs harness
//! ignored initial states, compared the DOCTYPE flag with the wrong polarity and never checked for
//! extra attributes, so its numbers meant nothing).
//!
//! Every test runs once per entry of its `initialStates` (default: the Data state) with its
//! `lastStartTag`; expected and actual character tokens are coalesced; the DOCTYPE's fifth field is
//! html5lib's "correctness", i.e. NOT force-quirks; attributes compare as exact sets; start tags
//! carry the self-closing flag; parse errors are not compared. Double-escaped tests whose strings
//! contain lone surrogates cannot be expressed as Rust `&str` and are counted as skipped.
//!
//! API this file relies on: `Tokenizer::with_state(input, InitialState, last_start_tag: Option<&str>)`
//! and `Tokenizer::tokenize()`; `InitialState { Data, Plaintext, Rcdata, Rawtext, ScriptData,
//! CdataSection }`. Test data: third_party/html5lib-tests (scripts/fetch-html5lib-tests.py).

use otter_html::{InitialState, Token, Tokenizer};
use otter_json::Value;

#[derive(Debug, PartialEq)]
enum Tok {
    Doctype { name: Option<String>, public: Option<String>, system: Option<String>, correct: bool },
    Start { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    End(String),
    Comment(String),
    Chars(String),
}

fn push_chars(out: &mut Vec<Tok>, s: &str) {
    if let Some(Tok::Chars(prev)) = out.last_mut() {
        prev.push_str(s);
    } else {
        out.push(Tok::Chars(s.to_string()));
    }
}

/// Decodes the literal `\uXXXX` sequences of a double-escaped test; None if a lone surrogate remains.
fn unescape(s: &str, double: bool) -> Option<String> {
    if !double {
        return Some(s.to_string());
    }
    let units: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let hex = |i: usize| -> Option<u32> {
        if i + 6 <= units.len() && units[i] == '\\' && units[i + 1] == 'u' {
            u32::from_str_radix(&units[i + 2..i + 6].iter().collect::<String>(), 16).ok()
        } else {
            None
        }
    };
    while i < units.len() {
        if let Some(u) = hex(i) {
            if (0xD800..0xDC00).contains(&u) {
                let lo = hex(i + 6).filter(|l| (0xDC00..0xE000).contains(l))?;
                out.push(char::from_u32(0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00))?);
                i += 12;
            } else {
                out.push(char::from_u32(u)?);
                i += 6;
            }
        } else {
            out.push(units[i]);
            i += 1;
        }
    }
    Some(out)
}

fn opt_str(v: Option<&Value>, double: bool) -> Option<Option<String>> {
    match v {
        Some(Value::String(s)) => Some(Some(unescape(s, double)?)),
        Some(Value::Null) | None => Some(None),
        _ => None,
    }
}

/// The expected token list, or None when the test cannot be represented (lone surrogates).
fn expected(output: &[Value], double: bool) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    for item in output {
        let Value::Array(a) = item else { panic!("malformed expected token {item:?}") };
        let kind = a[0].as_str().expect("token kind");
        let text = |i: usize| -> Option<String> { unescape(a.get(i)?.as_str()?, double) };
        match kind {
            "Character" => push_chars(&mut out, &text(1)?),
            "Comment" => out.push(Tok::Comment(text(1)?)),
            "EndTag" => out.push(Tok::End(text(1)?)),
            "StartTag" => {
                let mut attrs = Vec::new();
                if let Some(Value::Object(o)) = a.get(2) {
                    for (k, v) in o.iter() {
                        attrs.push((unescape(k, double)?, unescape(v.as_str()?, double)?));
                    }
                }
                attrs.sort();
                let self_closing = matches!(a.get(3), Some(Value::Bool(true)));
                out.push(Tok::Start { name: text(1)?, attrs, self_closing });
            }
            "DOCTYPE" => out.push(Tok::Doctype {
                name: opt_str(a.get(1), double)?,
                public: opt_str(a.get(2), double)?,
                system: opt_str(a.get(3), double)?,
                correct: matches!(a.get(4), Some(Value::Bool(true))),
            }),
            other => panic!("unknown expected token kind {other}"),
        }
    }
    Some(out)
}

fn actual(tokens: &[Token]) -> Vec<Tok> {
    let mut out = Vec::new();
    for t in tokens {
        match t {
            Token::Character(c) => push_chars(&mut out, c.encode_utf8(&mut [0u8; 4])),
            Token::Comment(s) => out.push(Tok::Comment(s.clone())),
            Token::EndTag { name } => out.push(Tok::End(name.clone())),
            Token::StartTag { name, attrs, self_closing } => {
                let mut a: Vec<(String, String)> = attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                a.sort();
                out.push(Tok::Start { name: name.clone(), attrs: a, self_closing: *self_closing });
            }
            Token::Doctype { name, public_id, system_id, force_quirks } => out.push(Tok::Doctype {
                name: name.clone(),
                public: public_id.clone(),
                system: system_id.clone(),
                correct: !force_quirks,
            }),
            Token::Eof => {}
        }
    }
    out
}

fn state(name: &str) -> InitialState {
    match name {
        "Data state" => InitialState::Data,
        "PLAINTEXT state" => InitialState::Plaintext,
        "RCDATA state" => InitialState::Rcdata,
        "RAWTEXT state" => InitialState::Rawtext,
        "Script data state" => InitialState::ScriptData,
        "CDATA section state" => InitialState::CdataSection,
        other => panic!("unknown initial state {other}"),
    }
}

/// Runs one html5lib tokenizer file; returns (passed, total, skipped) and prints the first failures.
fn run(file: &str) -> (usize, usize, usize) {
    let path = format!("{}/../../third_party/html5lib-tests/tokenizer/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => panic!("{path}: {e} (run scripts/fetch-html5lib-tests.py)"),
    };
    let doc = otter_json::parse(text.as_bytes()).unwrap_or_else(|e| panic!("{file}: JSON {e:?}"));
    let key = if doc.get("tests").is_some() { "tests" } else { "xmlViolationTests" };
    let tests = doc.get(key).and_then(Value::as_array).expect("tests array");
    let (mut passed, mut total, mut skipped, mut shown) = (0, 0, 0, 0);
    for t in tests {
        let double = matches!(t.get("doubleEscaped"), Some(Value::Bool(true)));
        let states: Vec<&str> = match t.get("initialStates").and_then(Value::as_array) {
            Some(a) => a.iter().map(|v| v.as_str().expect("state name")).collect(),
            None => vec!["Data state"],
        };
        let last = t.get("lastStartTag").and_then(Value::as_str);
        let raw_input = t.get("input").and_then(Value::as_str).expect("input");
        let output = t.get("output").and_then(Value::as_array).expect("output");
        for st in states {
            total += 1;
            let (Some(input), Some(want)) = (unescape(raw_input, double), expected(output, double)) else {
                skipped += 1;
                continue;
            };
            let got = actual(&Tokenizer::with_state(&input, state(st), last).tokenize());
            if got == want {
                passed += 1;
            } else if shown < 3 {
                shown += 1;
                let desc = t.get("description").and_then(Value::as_str).unwrap_or("");
                eprintln!("html5lib {file} [{st}] {desc:?}\n  input {input:?}\n  want  {want:?}\n  got   {got:?}");
            }
        }
    }
    (passed, total, skipped)
}

fn check(file: &str, min_percent: usize) {
    let (passed, total, skipped) = run(file);
    let counted = total - skipped;
    eprintln!("html5lib oracle: {file}: {passed}/{counted} passed ({skipped} skipped: lone surrogates)");
    assert!(counted > 0, "{file}: no runnable tests");
    assert!(passed * 100 >= counted * min_percent, "{file}: {passed}/{counted} is below {min_percent}%");
}

#[test]
fn html5lib_test1() {
    check("test1.test", 95);
}

#[test]
fn html5lib_test2() {
    check("test2.test", 95);
}

#[test]
fn html5lib_test3() {
    check("test3.test", 95);
}

#[test]
fn html5lib_test4() {
    check("test4.test", 95);
}

#[test]
fn html5lib_entities() {
    check("entities.test", 95);
}

#[test]
fn html5lib_named_entities() {
    check("namedEntities.test", 95);
}

#[test]
fn html5lib_numeric_entities() {
    check("numericEntities.test", 95);
}

#[test]
fn html5lib_content_model_flags() {
    check("contentModelFlags.test", 95);
}

#[test]
fn html5lib_escape_flag() {
    check("escapeFlag.test", 95);
}

#[test]
fn html5lib_domjs() {
    check("domjs.test", 95);
}

#[test]
fn html5lib_unicode_chars() {
    check("unicodeChars.test", 95);
}

#[test]
fn html5lib_unicode_chars_problematic() {
    check("unicodeCharsProblematic.test", 95);
}

#[test]
fn html5lib_pending_spec_changes() {
    check("pendingSpecChanges.test", 95);
}
