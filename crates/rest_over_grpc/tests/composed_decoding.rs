// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic, bounded decoder fuzzing and a replayable regression corpus.

use std::fmt::Write as _;

use rest_over_grpc::codegen_helpers::{RequestBodyKind, decode_request};
use rest_over_grpc::handling::Code;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Root {
    nested: Nested,
}

#[derive(Debug, Deserialize)]
struct Nested {
    value: String,
}

#[derive(Debug, Deserialize)]
struct Flat {
    value: String,
}

#[derive(Debug, Deserialize)]
struct Repeated {
    nested: RepeatedNested,
}

#[derive(Debug, Deserialize)]
struct RepeatedNested {
    value: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    Flat,
    Nested,
    Repeated,
}

#[derive(Debug)]
struct Seed {
    name: &'static str,
    shape: Shape,
    kind: RequestBodyKind,
    query: &'static [(&'static str, &'static str)],
    body: &'static [u8],
    expected: Result<&'static str, Code>,
}

// Named, saved cases: any failure reports the name and can be replayed alone.
const SEEDS: &[Seed] = &[
    Seed {
        name: "escaped-nested-value-and-plus",
        shape: Shape::Nested,
        kind: RequestBodyKind::None,
        query: &[("nested.value", "science+%66iction")],
        body: b"",
        expected: Ok("science fiction"),
    },
    Seed {
        name: "encoded-nested-name-and-value",
        shape: Shape::Nested,
        kind: RequestBodyKind::Whole,
        query: &[("nested%2Evalue", "science+%66iction")],
        body: br#"{"nested":{"value":"body"}}"#,
        expected: Ok("science fiction"),
    },
    Seed {
        name: "flat-value-and-plus",
        shape: Shape::Flat,
        kind: RequestBodyKind::Whole,
        query: &[("value", "a%2Bb+c")],
        body: br#"{"value":"body"}"#,
        expected: Ok("a+b c"),
    },
    Seed {
        name: "repeated-nested-sequence",
        shape: Shape::Repeated,
        kind: RequestBodyKind::None,
        query: &[("nested.value", "first"), ("nested.value", "second")],
        body: b"",
        expected: Ok("first|second"),
    },
    Seed {
        name: "repeated-nested-scalar",
        shape: Shape::Nested,
        kind: RequestBodyKind::None,
        query: &[("nested.value", "first"), ("nested.value", "second")],
        body: b"",
        expected: Err(Code::InvalidArgument),
    },
    Seed {
        name: "malformed-nested-escape",
        shape: Shape::Nested,
        kind: RequestBodyKind::None,
        query: &[("nested.value", "%2x")],
        body: b"",
        expected: Err(Code::InvalidArgument),
    },
    Seed {
        name: "malformed-nested-name",
        shape: Shape::Nested,
        kind: RequestBodyKind::None,
        query: &[("nested%zzvalue", "ok")],
        body: b"",
        expected: Err(Code::InvalidArgument),
    },
    Seed {
        name: "invalid-utf8-in-nested-value",
        shape: Shape::Nested,
        kind: RequestBodyKind::None,
        query: &[("nested.value", "%FF")],
        body: b"",
        expected: Err(Code::InvalidArgument),
    },
    Seed {
        name: "conflicting-query-paths",
        shape: Shape::Nested,
        kind: RequestBodyKind::None,
        query: &[("nested", "scalar"), ("nested.value", "child")],
        body: b"",
        expected: Err(Code::InvalidArgument),
    },
    Seed {
        name: "query-overrides-nested-body",
        shape: Shape::Nested,
        kind: RequestBodyKind::Whole,
        query: &[("nested.value", "query")],
        body: br#"{"nested":{"value":"body"}}"#,
        expected: Ok("query"),
    },
    Seed {
        name: "query-replaces-null-field-body",
        shape: Shape::Nested,
        kind: RequestBodyKind::Field("nested"),
        query: &[("nested.value", "query")],
        body: b"null",
        expected: Ok("query"),
    },
    Seed {
        name: "non-object-nested-body-conflict",
        shape: Shape::Nested,
        kind: RequestBodyKind::Whole,
        query: &[("nested.value", "query")],
        body: br#"{"nested":"scalar"}"#,
        expected: Err(Code::InvalidArgument),
    },
    Seed {
        name: "duplicate-body-field",
        shape: Shape::Flat,
        kind: RequestBodyKind::Whole,
        query: &[("value", "query")],
        body: br#"{"value":"a","value":"b"}"#,
        expected: Err(Code::InvalidArgument),
    },
    Seed {
        name: "malformed-json-body",
        shape: Shape::Nested,
        kind: RequestBodyKind::Whole,
        query: &[("nested.value", "query")],
        body: br#"{"nested":{"value":"body"}"#,
        expected: Err(Code::InvalidArgument),
    },
];

fn decode_seed(shape: Shape, query: &[(&str, &str)], body: &[u8], kind: RequestBodyKind) -> Result<String, Code> {
    match shape {
        Shape::Flat => decode_request::<Flat>(query, body, kind)
            .map(|decoded| decoded.value)
            .map_err(|error| error.code()),
        Shape::Nested => decode_request::<Root>(query, body, kind)
            .map(|decoded| decoded.nested.value)
            .map_err(|error| error.code()),
        Shape::Repeated => decode_request::<Repeated>(query, body, kind)
            .map(|decoded| decoded.nested.value.join("|"))
            .map_err(|error| error.code()),
    }
}

#[test]
fn saved_seeds_replay_with_exact_values_and_classified_failures() {
    for seed in SEEDS {
        for replay in 0..2 {
            let actual = decode_seed(seed.shape, seed.query, seed.body, seed.kind);
            assert_eq!(
                actual.as_deref(),
                seed.expected.as_ref().map(|expected| *expected),
                "{} replay {replay}",
                seed.name
            );
        }
    }
}

// A tiny fixed-state generator makes fuzz cases reproducible without an
// external RNG or persistent generated files.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn encode_query_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' => encoded.push(char::from(byte)),
            b' ' => encoded.push('+'),
            _ => write!(encoded, "%{byte:02X}").expect("writing to a String cannot fail"),
        }
    }
    encoded
}

// Fixed seeds, fixed case count and maximum input length; a failing seed and
// iteration can be replayed by retaining just that loop iteration.
const FUZZ_SEEDS: &[u64] = &[0x54f2_981a_3b6c_704d, 0xfedc_ba98_7654_3210, 0x1a2b_3c4d_5e6f_7081];
const CASES_PER_SEED: usize = 64;

#[test]
fn bounded_generated_cases_round_trip_flat_and_nested_overlays() {
    const ALPHABET: &[char] = &['a', 'Z', '0', ' ', '+', '%', '/', 'é'];

    for &seed in FUZZ_SEEDS {
        let mut state = seed;
        for iteration in 0..CASES_PER_SEED {
            let length = 1 + usize::try_from(next(&mut state) % 24).unwrap();
            let value: String = (0..length)
                .map(|_| ALPHABET[usize::try_from(next(&mut state) % ALPHABET.len() as u64).unwrap()])
                .collect();
            let encoded = encode_query_value(&value);
            let mode = usize::try_from(next(&mut state) % 3).unwrap();
            let kind = match mode {
                0 => RequestBodyKind::None,
                1 => RequestBodyKind::Whole,
                _ => RequestBodyKind::Field("nested"),
            };
            let body = match mode {
                0 => b"".as_slice(),
                1 => br#"{"nested":{"value":"body"}}"#.as_slice(),
                _ => br#"{"value":"body"}"#.as_slice(),
            };
            let name = if next(&mut state) & 1 == 0 {
                "nested.value"
            } else {
                "nested%2Evalue"
            };
            let actual = decode_seed(Shape::Nested, &[(name, &encoded)], body, kind);
            assert_eq!(
                actual.as_deref(),
                Ok(value.as_str()),
                "seed {seed:#x}, iteration {iteration}, nested"
            );

            let flat = decode_seed(Shape::Flat, &[("value", &encoded)], br#"{"value":"body"}"#, RequestBodyKind::Whole);
            assert_eq!(flat.as_deref(), Ok(value.as_str()), "seed {seed:#x}, iteration {iteration}, flat");
        }
    }
}

#[test]
fn dotted_field_depth_boundary_is_replayable() {
    // 64 segments are accepted; a 65th segment is rejected during insertion
    // rather than traversing beyond the decoder's recursion budget.
    let name = format!("{}value", "branch.".repeat(63));
    let decoded: serde_json::Value = decode_request(&[(&name, "leaf")], b"", RequestBodyKind::None).unwrap();
    let path = format!("/{}value", "branch/".repeat(63));
    assert_eq!(decoded.pointer(&path).and_then(serde_json::Value::as_str), Some("leaf"));

    let too_deep = format!("branch.{name}");
    for _ in 0..2 {
        let error = decode_request::<serde_json::Value>(&[(&too_deep, "leaf")], b"", RequestBodyKind::None).unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
    }
}

#[test]
fn direct_decode_enforces_query_work_budget_without_a_bounded_parser() {
    let at_count_limit = vec![("nested.value", "x"); 128];
    let values: Result<Repeated, _> = decode_request(&at_count_limit, b"", RequestBodyKind::None);
    assert_eq!(values.unwrap().nested.value.len(), 128);

    let over_count = vec![("nested.value", "x"); 129];
    for kind in [RequestBodyKind::None, RequestBodyKind::Whole, RequestBodyKind::Field("nested")] {
        let error = decode_request::<Root>(&over_count, b"{", kind).unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        assert!(error.to_string().contains("query exceeds"), "{error}");
    }

    let at_key_limit = "k".repeat(4096);
    let _: serde_json::Value = decode_request(&[(&at_key_limit, "ok")], b"", RequestBodyKind::None).unwrap();
    let over_key = format!("{at_key_limit}k");
    let error = decode_request::<serde_json::Value>(&[(&over_key, "ok")], b"", RequestBodyKind::None).unwrap_err();
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(error.to_string().contains("query exceeds"), "{error}");

    let at_value_limit = "v".repeat(16 * 1024 - "value".len());
    let decoded: Flat = decode_request(&[("value", &at_value_limit)], b"", RequestBodyKind::None).unwrap();
    assert_eq!(decoded.value.len(), at_value_limit.len());
    let over_value = format!("{at_value_limit}v");
    let error = decode_request::<Flat>(&[("value", &over_value)], b"", RequestBodyKind::None).unwrap_err();
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(error.to_string().contains("query exceeds"), "{error}");
}
