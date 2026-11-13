// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Query decoding, serialization, and limit regression oracles.

#![cfg(feature = "query")]

use std::borrow::Cow;
use std::fmt::{self, Write};

use routerama::query::{ErrorKind, FromQuery, QueryLimits, ToQuery};

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(deny_unknown_fields)]
struct Query<'q> {
    #[query(rename = "k", alias = "alias")]
    key: Cow<'q, str>,
    #[query(default)]
    values: Vec<String>,
}

#[test]
fn gamma_survivor_query_decoding_scalar_and_wide() {
    for padding in [0, 15, 16, 31, 32, 39, 40, 64] {
        let value = format!("{}%00%09%2b+%3D%26%25%C3%A9tail", "x".repeat(padding));
        let input = format!("&&%6B={value}&&values=one=two&values&");
        let parsed = Query::from_query(&input).unwrap();
        assert_eq!(parsed.key, format!("{}\0\t+ =&%étail", "x".repeat(padding)));
        assert!(matches!(parsed.key, Cow::Owned(_)));
        assert_eq!(parsed.values, ["one=two", ""]);
    }
    let parsed = Query::from_query("k=plain").unwrap();
    assert!(matches!(parsed.key, Cow::Borrowed("plain")));
    assert!(parsed.values.is_empty());
    assert_eq!(Query::from_query("k").unwrap().key, "");
}

#[test]
fn gamma_survivor_query_errors_have_exact_offsets_and_categories() {
    for (input, offset, kind, parameter) in [
        ("k=x&alias=y", 4, ErrorKind::Duplicate, Some("k")),
        ("k=x&other=y", 4, ErrorKind::UnknownParameter, None),
        ("k=x&values=%", 11, ErrorKind::InvalidEncoding, None),
        ("k=x&values=%0", 11, ErrorKind::InvalidEncoding, None),
        ("k=x&values=%G0", 11, ErrorKind::InvalidEncoding, None),
        ("k=x&values=%0G", 11, ErrorKind::InvalidEncoding, None),
        ("k=%FF", 2, ErrorKind::InvalidUtf8, None),
        ("k=%C3%A9%G0", 8, ErrorKind::InvalidEncoding, None),
        ("k=%C3%A9%0G", 8, ErrorKind::InvalidEncoding, None),
        ("k=%C3%A9%", 8, ErrorKind::InvalidEncoding, None),
        ("k=%C3%A9%0", 8, ErrorKind::InvalidEncoding, None),
        ("values=x", 8, ErrorKind::Missing, Some("k")),
    ] {
        let error = Query::from_query(input).unwrap_err();
        assert_eq!(
            (error.pair_offset(), error.kind(), error.parameter()),
            (Some(offset), kind, parameter),
            "{input}"
        );
        let suffix = parameter.map_or_else(String::new, |name| format!(" for parameter `{name}`"));
        assert_eq!(error.to_string(), format!("query parsing failed at byte {offset}: {kind}{suffix}"));
        assert!(std::error::Error::source(&error).is_none());
    }
    for padding in [0, 40] {
        let input = format!("k={}x&values=%C3%A9+z%2Btail", "a".repeat(padding));
        assert_eq!(Query::from_query(&input).unwrap().values, ["é z+tail"]);
        let bad = format!("k={}x&%G0=z", "a".repeat(padding));
        assert_eq!(Query::from_query(&bad).unwrap_err().pair_offset(), Some(padding + 4));
    }
}

#[test]
fn gamma_survivor_query_resource_limits_are_inclusive() {
    let input = "k=a&values=%41&values=b";
    let limits = QueryLimits {
        max_query_length: input.len(),
        max_pairs: 3,
        max_decoded_length: 16,
        max_repeated_values: 2,
        ..QueryLimits::UNLIMITED
    };
    assert_eq!(Query::from_query_with(input, limits).unwrap().values, ["A", "b"]);
    for (limits, kind, offset) in [
        (
            QueryLimits {
                max_query_length: input.len() - 1,
                ..limits
            },
            ErrorKind::QueryTooLong,
            0,
        ),
        (QueryLimits { max_pairs: 2, ..limits }, ErrorKind::TooManyPairs, 15),
        (
            QueryLimits {
                max_decoded_length: 15,
                ..limits
            },
            ErrorKind::DecodedTooLong,
            22,
        ),
        (
            QueryLimits {
                max_repeated_values: 1,
                ..limits
            },
            ErrorKind::TooManyValues,
            15,
        ),
    ] {
        let error = Query::from_query_with(input, limits).unwrap_err();
        assert_eq!((error.kind(), error.pair_offset()), (kind, Some(offset)));
    }
    assert_eq!(
        Query::from_query_with(
            "&&k=a&&",
            QueryLimits {
                max_pairs: 1,
                max_decoded_length: 2,
                ..QueryLimits::UNLIMITED
            }
        )
        .unwrap()
        .key,
        "a"
    );
}

struct Chunks;
impl fmt::Display for Chunks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a/")?;
        f.write_str("é +")?;
        f.write_str("z")
    }
}

struct Fails;
impl fmt::Display for Fails {
    fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[derive(ToQuery)]
struct Output<T> {
    value: T,
    flag: bool,
    signed: isize,
    unsigned: usize,
}

struct RejectAfter {
    output: String,
    remaining: usize,
}
impl Write for RejectAfter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.remaining == 0 {
            return Err(fmt::Error);
        }
        self.remaining -= 1;
        self.output.push_str(s);
        Ok(())
    }
}

#[test]
fn gamma_survivor_query_streaming_encoding_and_failures() {
    let output = Output {
        value: Chunks,
        flag: false,
        signed: -17,
        unsigned: 23,
    };
    let expected = "value=a%2F%C3%A9+%2Bz&flag=false&signed=-17&unsigned=23";
    assert_eq!(output.to_query_string().unwrap(), expected);
    assert_eq!(
        output
            .to_query_string_with(QueryLimits {
                max_encoded_length: expected.len(),
                ..QueryLimits::UNLIMITED
            })
            .unwrap(),
        expected
    );
    let error = output
        .to_query_string_with(QueryLimits {
            max_encoded_length: expected.len() - 1,
            ..QueryLimits::UNLIMITED
        })
        .unwrap_err();
    assert_eq!(
        (error.kind(), error.parameter(), error.pair_offset()),
        (ErrorKind::TooLong, Some("unsigned"), None)
    );
    assert_eq!(
        error.to_string(),
        "query production failed: encoded output exceeds the configured length limit for parameter `unsigned`"
    );
    for remaining in 0..21 {
        let mut writer = RejectAfter {
            output: String::new(),
            remaining,
        };
        let error = output.write_query(&mut writer).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Output);
        assert!(error.parameter().is_some());
    }

    let error = Output {
        value: Fails,
        flag: true,
        signed: 0,
        unsigned: 0,
    }
    .to_query_string()
    .unwrap_err();
    assert_eq!((error.kind(), error.parameter()), (ErrorKind::Format, Some("value")));
    let query = Query {
        key: Cow::Borrowed("AZaz09*-._ /+&=~\0"),
        values: vec!["first".into(), "second".into()],
    };
    assert_eq!(
        query.to_query_string().unwrap(),
        "k=AZaz09*-._+%2F%2B%26%3D%7E%00&values=first&values=second"
    );
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(rename_all = "camelCase")]
struct Camel<'q> {
    user_name: &'q str,
    optional_count: Option<u32>,
    repeated_count: Vec<u32>,
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(rename_all = "kebab-case")]
struct Kebab {
    user_name: String,
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(rename_all = "SCREAMING_SNAKE_CASE")]
struct Screaming {
    user_name: String,
}

#[test]
fn gamma_survivor_derived_renames_borrowed_errors_and_numeric_values() {
    let camel = Camel::from_query("userName=raw&optionalCount=42&repeatedCount=1&repeatedCount=2").unwrap();
    assert_eq!(
        camel,
        Camel {
            user_name: "raw",
            optional_count: Some(42),
            repeated_count: vec![1, 2]
        }
    );
    assert_eq!(
        camel.to_query_string().unwrap(),
        "userName=raw&optionalCount=42&repeatedCount=1&repeatedCount=2"
    );
    assert_eq!(
        Camel {
            user_name: "raw",
            optional_count: None,
            repeated_count: vec![]
        }
        .to_query_string()
        .unwrap(),
        "userName=raw"
    );
    let error = Camel::from_query("repeatedCount=1&userName=has+space").unwrap_err();
    assert_eq!(
        (error.kind(), error.pair_offset(), error.parameter()),
        (ErrorKind::BorrowRequired, Some(16), Some("userName"))
    );
    let error = Camel::from_query("userName=x&optionalCount=bad").unwrap_err();
    assert_eq!(
        (error.kind(), error.pair_offset(), error.parameter()),
        (ErrorKind::InvalidValue, Some(11), Some("optionalCount"))
    );
    let error = Camel::from_query("userName=x&optionalCount=1&optionalCount=2").unwrap_err();
    assert_eq!(
        (error.kind(), error.pair_offset(), error.parameter()),
        (ErrorKind::Duplicate, Some(27), Some("optionalCount"))
    );
    assert_eq!(
        Kebab::from_query("user-name=hello").unwrap().to_query_string().unwrap(),
        "user-name=hello"
    );
    assert_eq!(
        Screaming::from_query("USER_NAME=hello").unwrap().to_query_string().unwrap(),
        "USER_NAME=hello"
    );
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(deny_unknown_fields)]
struct Left {
    left: Option<u32>,
}
#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct Right {
    right: Vec<u32>,
}
#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct Flattened {
    root: String,
    #[query(flatten)]
    left: Left,
    #[query(flatten)]
    right: Right,
}
#[derive(Debug, FromQuery)]
struct Ambiguous {
    #[query(flatten)]
    first: Left,
    #[query(flatten)]
    second: Left,
}
#[derive(Debug, FromQuery)]
struct DirectAmbiguous {
    left: u32,
    #[query(flatten)]
    nested: Left,
}

#[test]
fn gamma_survivor_flatten_dispatch_claims_and_ambiguity() {
    let decoded = Flattened::from_query("root=value&right=3&left=7&right=4").unwrap();
    assert_eq!(
        decoded,
        Flattened {
            root: "value".into(),
            left: Left { left: Some(7) },
            right: Right { right: vec![3, 4] }
        }
    );
    assert_eq!(decoded.to_query_string().unwrap(), "root=value&left=7&right=3&right=4");
    let error = Flattened::from_query("root=value&unknown=1").unwrap_err();
    assert_eq!((error.kind(), error.pair_offset()), (ErrorKind::UnknownParameter, Some(11)));
    for error in [
        Ambiguous::from_query("left=7").unwrap_err(),
        DirectAmbiguous::from_query("left=7").unwrap_err(),
    ] {
        assert_eq!(
            (error.kind(), error.pair_offset(), error.parameter()),
            (ErrorKind::AmbiguousParameter, Some(0), None)
        );
    }
    let empty = Ambiguous::from_query("").unwrap();
    assert_eq!((empty.first.left, empty.second.left), (None, None));
    let direct = DirectAmbiguous {
        left: 7,
        nested: Left { left: None },
    };
    assert_eq!((direct.left, direct.nested.left), (7, None));
    let error = Flattened::from_query("root=value&right=bad").unwrap_err();
    assert_eq!(
        (error.kind(), error.pair_offset(), error.parameter()),
        (ErrorKind::InvalidValue, Some(11), Some("right"))
    );
}

#[test]
fn gamma_survivor_wide_query_limit_and_encoding_error_offsets() {
    let first = format!("k={}", "a".repeat(40));
    let input = format!("{first}&values=%C3%A9");
    let decoded_length = 49;
    let limits = QueryLimits {
        max_pairs: 2,
        max_decoded_length: decoded_length,
        ..QueryLimits::UNLIMITED
    };
    assert_eq!(Query::from_query_with(&input, limits).unwrap().values, ["é"]);
    for (limits, kind, offset) in [
        (QueryLimits { max_pairs: 1, ..limits }, ErrorKind::TooManyPairs, 43),
        (
            QueryLimits {
                max_decoded_length: decoded_length - 1,
                ..limits
            },
            ErrorKind::DecodedTooLong,
            50,
        ),
    ] {
        let error = Query::from_query_with(&input, limits).unwrap_err();
        assert_eq!((error.kind(), error.pair_offset()), (kind, Some(offset)));
    }
    let plain = format!("{first}&values=long-plain");
    let error = Query::from_query_with(
        &plain,
        QueryLimits {
            max_decoded_length: 41,
            ..QueryLimits::UNLIMITED
        },
    )
    .unwrap_err();
    assert_eq!((error.kind(), error.pair_offset()), (ErrorKind::DecodedTooLong, Some(43)));
    let bad = format!("{first}&values=%GG");
    assert_eq!(Query::from_query(&bad).unwrap_err().pair_offset(), Some(50));
    let bad = format!("{first}&values=%FF");
    assert_eq!(Query::from_query(&bad).unwrap_err().pair_offset(), Some(50));
    let bad_key = format!("{first}&%GG=x");
    assert_eq!(Query::from_query(&bad_key).unwrap_err().pair_offset(), Some(43));
}

#[test]
fn gamma_survivor_query_error_display_propagates_formatter_errors() {
    let parsing = Query::from_query("values=x").unwrap_err();
    let production = Output {
        value: Fails,
        flag: false,
        signed: 0,
        unsigned: 0,
    }
    .to_query_string()
    .unwrap_err();
    for error in [parsing, production] {
        for remaining in 0..4 {
            let mut writer = RejectAfter {
                output: String::new(),
                remaining,
            };
            assert!(fmt::write(&mut writer, format_args!("{error}")).is_err());
        }
    }
}

#[test]
fn gamma_survivor_query_plus_only_and_plain_simd_boundaries() {
    for length in [16, 31, 32, 40, 41, 65] {
        for marker in [0, 15, 16, length - 1] {
            if marker >= length {
                continue;
            }
            let mut value = "x".repeat(length);
            value.replace_range(marker..=marker, "+");
            let input = format!("k={value}&values=tail");
            let parsed = Query::from_query(&input).unwrap();
            assert_eq!(parsed.key, value.replace('+', " "));
            assert!(matches!(parsed.key, Cow::Owned(_)));
            assert_eq!(parsed.values, ["tail"]);
        }
        let input = format!("k={}&values=tail", "x".repeat(length));
        let parsed = Query::from_query(&input).unwrap();
        assert_eq!(parsed.key, "x".repeat(length));
        assert!(matches!(parsed.key, Cow::Borrowed(_)));
        assert_eq!(parsed.values, ["tail"]);
    }
    for (padding, length, expected_offset) in [(19, 39, 29), (20, 40, 30), (21, 41, 31)] {
        let input = format!("k={}&values=long-plain", "a".repeat(padding));
        assert_eq!(input.len(), length);
        let error = Query::from_query_with(
            &input,
            QueryLimits {
                max_decoded_length: length - 4,
                ..QueryLimits::UNLIMITED
            },
        )
        .unwrap_err();
        assert_eq!(
            (error.kind(), error.pair_offset()),
            (ErrorKind::DecodedTooLong, Some(expected_offset))
        );
    }
}
