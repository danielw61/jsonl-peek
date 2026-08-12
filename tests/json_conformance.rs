//! Acceptance and rejection cases for the JSON parser, driven through the
//! public API of the library.

use jsonl_peek::json::{parse, ErrorKind, Value};

/// Documents that must parse, with the compact form they must serialise back to.
const ACCEPT: &[(&str, &str)] = &[
    ("{}", "{}"),
    ("[]", "[]"),
    ("  null  ", "null"),
    ("[true,false,null]", "[true,false,null]"),
    (r#"{"a":1,"b":[2,{"c":3}]}"#, r#"{"a":1,"b":[2,{"c":3}]}"#),
    ("[-0.5e-2]", "[-0.005]"),
    ("[1e2]", "[100.0]"),
    (r#"["Aé世"]"#, r#"["Aé世"]"#),
    (r#"["tab\there"]"#, r#"["tab\there"]"#),
    ("[9223372036854775807]", "[9223372036854775807]"),
    ("{\n  \"pretty\" : true\n}", r#"{"pretty":true}"#),
    (r#"{"":"empty key"}"#, r#"{"":"empty key"}"#),
];

/// Documents that must be rejected, with the expected failure.
const REJECT: &[(&str, ErrorKind)] = &[
    ("", ErrorKind::UnexpectedEof),
    ("{", ErrorKind::UnexpectedEof),
    ("}", ErrorKind::UnexpectedByte(b'}')),
    ("[1,2", ErrorKind::UnexpectedEof),
    ("[1,2,]", ErrorKind::UnexpectedByte(b']')),
    (r#"{"a":1,}"#, ErrorKind::ExpectedKey),
    (r#"{"a"}"#, ErrorKind::ExpectedColon),
    (r#"{"a":}"#, ErrorKind::UnexpectedByte(b'}')),
    ("[01]", ErrorKind::ExpectedCommaOrCloseBracket),
    ("[.5]", ErrorKind::UnexpectedByte(b'.')),
    ("[1.]", ErrorKind::InvalidNumber),
    ("[--1]", ErrorKind::InvalidNumber),
    ("[Infinity]", ErrorKind::UnexpectedByte(b'I')),
    ("[nul]", ErrorKind::InvalidLiteral),
    ("[TRUE]", ErrorKind::UnexpectedByte(b'T')),
    ("'single'", ErrorKind::UnexpectedByte(b'\'')),
    ("null null", ErrorKind::TrailingData),
    (r#""\uD800""#, ErrorKind::UnpairedSurrogate),
    (r#""\uD800A""#, ErrorKind::UnpairedSurrogate),
    (r#""\q""#, ErrorKind::InvalidEscape(b'q')),
    (r#""\u00g0""#, ErrorKind::InvalidUnicodeEscape),
];

#[test]
fn accepts_valid_documents() {
    for (input, expected) in ACCEPT {
        let value = parse(input).unwrap_or_else(|err| panic!("{:?} should parse: {}", input, err));
        assert_eq!(&value.to_json(), expected, "round trip of {:?}", input);
    }
}

#[test]
fn rejects_invalid_documents() {
    for (input, expected) in REJECT {
        let err = parse(input).unwrap_err();
        assert_eq!(&err.kind, expected, "wrong error for {:?}", input);
        assert!(
            err.offset <= input.len(),
            "offset {} outside {:?}",
            err.offset,
            input
        );
    }
}

#[test]
fn reparsing_the_output_is_stable() {
    for (input, _) in ACCEPT {
        let once = parse(input).unwrap();
        let twice = parse(&once.to_json()).unwrap();
        assert_eq!(once, twice, "not idempotent for {:?}", input);
    }
}

#[test]
fn keeps_integers_and_floats_apart() {
    assert_eq!(parse("1").unwrap().type_name(), "int");
    assert_eq!(parse("1.0").unwrap().type_name(), "float");
    assert_eq!(parse("1e0").unwrap().type_name(), "float");
    assert_eq!(parse("-9223372036854775808").unwrap(), Value::Int(i64::MIN));
    // One past i64::MAX no longer fits and degrades to a float.
    assert_eq!(parse("9223372036854775808").unwrap().type_name(), "float");
}

#[test]
fn error_offsets_point_at_the_problem() {
    let err = parse(r#"{"ok":1,"bad":tru}"#).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidLiteral);
    assert_eq!(err.offset, 14);
    assert!(err.to_string().starts_with("at byte 14:"));
}

#[test]
fn deeply_nested_input_fails_instead_of_crashing() {
    let payload = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
    let err = parse(&payload).unwrap_err();
    assert_eq!(err.kind, ErrorKind::DepthLimitExceeded);
}
