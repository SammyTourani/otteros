//! Parse/serialize round-trip and DOM-construction tests (brief M7-T1).

use otter_json::{Number, Object, Value, parse, parse_str, to_string, to_string_pretty};

#[test]
fn compact_roundtrip_of_a_representative_document() {
    let src = r#"{"a":1,"b":[true,false,null],"c":"hi\nthere","d":{"e":-3.5}}"#;
    let v = parse_str(src).expect("parse");
    let out = to_string(&v);
    let v2 = parse_str(&out).expect("reparse");
    assert_eq!(v, v2);
}

#[test]
fn pretty_output_is_indented_and_reparses_to_the_same_value() {
    let src = r#"{"a":[1,2,{"b":true}],"c":{}, "d": []}"#;
    let v = parse_str(src).expect("parse");
    let pretty = to_string_pretty(&v);
    assert!(pretty.contains("\n  \"a\""), "expected 2-space indent, got:\n{pretty}");
    assert!(pretty.contains("\n    {\n      \"b\": true\n    }"), "nested object indents one level deeper:\n{pretty}");
    let v2 = parse_str(&pretty).expect("reparse pretty");
    assert_eq!(v, v2);
    // Empty containers never wrap onto their own lines.
    assert!(pretty.contains("\"c\": {}"));
    assert!(pretty.contains("\"d\": []"));
}

#[test]
fn object_preserves_insertion_order_and_last_value_wins_on_duplicates() {
    let v = parse_str(r#"{"z":1,"a":2,"z":3}"#).expect("parse");
    let obj = v.as_object().expect("object");
    let keys: Vec<&str> = obj.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["z", "a", "z"], "source order preserved, including the duplicate");
    assert_eq!(obj.get("z"), Some(&Value::Number(Number::Int(3))), "last duplicate wins");
    // Round-tripping through the serializer preserves the duplicate too.
    assert_eq!(to_string(&v), r#"{"z":1,"a":2,"z":3}"#);
}

#[test]
fn object_insert_replaces_last_matching_key_in_place() {
    let mut obj = Object::new();
    obj.insert("a".into(), Value::from(1i64));
    obj.insert("b".into(), Value::from(2i64));
    let old = obj.insert("a".into(), Value::from(9i64));
    assert_eq!(old, Some(Value::from(1i64)));
    let keys: Vec<&str> = obj.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["a", "b"], "replacing a key keeps its original position");
    assert_eq!(obj.get("a"), Some(&Value::from(9i64)));
}

#[test]
fn integers_stay_exact_across_i64_and_u64_boundaries() {
    assert_eq!(parse_str("0").unwrap(), Value::Number(Number::Int(0)));
    assert_eq!(parse_str("-0").unwrap(), Value::Number(Number::Int(0)));
    assert_eq!(parse_str("9223372036854775807").unwrap(), Value::Number(Number::Int(i64::MAX)));
    assert_eq!(parse_str("-9223372036854775808").unwrap(), Value::Number(Number::Int(i64::MIN)));
    // One past i64::MAX: must not silently become a lossy float.
    assert_eq!(parse_str("9223372036854775808").unwrap(), Value::Number(Number::UInt(9223372036854775808)));
    assert_eq!(parse_str("18446744073709551615").unwrap(), Value::Number(Number::UInt(u64::MAX)));
    // A literal with a decimal point is always a Float even if integral.
    assert_eq!(parse_str("5.0").unwrap(), Value::Number(Number::Float(5.0)));
    assert!(matches!(parse_str("5.0").unwrap().as_number().unwrap(), Number::Float(_)));
}

#[test]
fn number_accessors_convert_between_representations() {
    assert_eq!(Number::Int(-5).as_i64(), Some(-5));
    assert_eq!(Number::Int(-5).as_u64(), None);
    assert_eq!(Number::UInt(10).as_i64(), Some(10));
    assert_eq!(Number::Float(3.0).as_i64(), Some(3));
    assert_eq!(Number::Float(3.5).as_i64(), None);
    assert_eq!(Number::Int(7).as_f64(), 7.0);
}

#[test]
fn float_formatting_round_trips_for_a_wide_sample_of_magnitudes() {
    let samples: &[f64] = &[
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.1,
        1.5,
        123.456,
        1e21,
        1e20,
        1e-4,
        1e-5,
        1e-7,
        2.5e-6,
        1234567890123456.0,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::EPSILON,
        core::f64::consts::PI,
    ];
    for &x in samples {
        let v = Value::from(x);
        let s = to_string(&v);
        let back = parse_str(&s).expect("reparse float");
        assert_eq!(back.as_f64(), Some(x), "round trip of {x} through {s:?} (parsed as {back:?})");
    }
}

#[test]
fn non_finite_floats_serialize_as_null_not_invalid_json() {
    assert_eq!(to_string(&Value::from(f64::NAN)), "null");
    assert_eq!(to_string(&Value::from(f64::INFINITY)), "null");
    assert_eq!(to_string(&Value::from(f64::NEG_INFINITY)), "null");
}

#[test]
fn string_escapes_round_trip_including_surrogate_pairs_and_controls() {
    // "😀" is U+1F600 GRINNING FACE, a classic surrogate-pair case.
    let v = parse_str(r#""😀 tab:\t nl:\n quote:\" back:\\ bell:\u0007""#).expect("parse");
    let s = v.as_str().expect("string");
    assert_eq!(s, "\u{1F600} tab:\t nl:\n quote:\" back:\\ bell:\u{0007}");
    let out = to_string(&v);
    assert_eq!(parse_str(&out).unwrap(), v);
    // The astral character is emitted as raw UTF-8, not re-escaped as a surrogate pair.
    assert!(out.contains('\u{1F600}'));
}

#[test]
fn lone_surrogates_become_replacement_character_not_a_crash() {
    let v = parse_str(r#""\uD800""#).expect("lone high surrogate is accepted (implementation-defined)");
    assert_eq!(v.as_str(), Some("\u{FFFD}"));
    let v = parse_str(r#""\uDC00""#).expect("lone low surrogate is accepted (implementation-defined)");
    assert_eq!(v.as_str(), Some("\u{FFFD}"));
}

#[test]
fn depth_limit_rejects_pathological_nesting_without_recursing_forever() {
    let deep = "[".repeat(otter_json::MAX_DEPTH + 1) + &"]".repeat(otter_json::MAX_DEPTH + 1);
    assert!(parse_str(&deep).is_err());
    // One level inside the limit still parses fine.
    let ok = "[".repeat(otter_json::MAX_DEPTH - 1) + "1" + &"]".repeat(otter_json::MAX_DEPTH - 1);
    assert!(parse_str(&ok).is_ok());
}

#[test]
fn value_helper_accessors() {
    let v = parse_str(r#"{"n":42,"s":"hi","a":[1,2],"b":true,"nil":null}"#).unwrap();
    assert_eq!(v.get("n").and_then(|n| n.as_i64()), Some(42));
    assert_eq!(v.get("s").and_then(|s| s.as_str()), Some("hi"));
    assert_eq!(v.get("a").and_then(|a| a.index(1)).and_then(|x| x.as_i64()), Some(2));
    assert_eq!(v.get("b").and_then(|b| b.as_bool()), Some(true));
    assert!(v.get("nil").unwrap().is_null());
    assert!(v.get("missing").is_none());
}

#[test]
fn parse_bytes_and_parse_str_agree() {
    let src = br#"{"x":1}"#;
    let a = parse(src).unwrap();
    let b = parse_str(core::str::from_utf8(src).unwrap()).unwrap();
    assert_eq!(a, b);
}
