// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Errors whose offending pair remains on the wide parser path.

#![cfg(feature = "query")]

use routerama::query::{ErrorKind, FromQuery, QueryLimits};

#[derive(Debug, FromQuery)]
struct Values {
    lead: String,
    item: String,
    tail: String,
}

#[derive(Debug, FromQuery)]
struct Borrowed<'q> {
    item: &'q str,
}

#[test]
fn gamma_nov4_wide_pair_limit_reports_the_exact_rejected_pair_start_after_every_separator() {
    let pairs = [
        format!("lead={}", "a".repeat(80)),
        format!("item={}", "b".repeat(80)),
        format!("tail={}", "c".repeat(80)),
    ];
    let input = pairs.join("&");
    let mut offset = 0;
    for (maximum, pair) in pairs.iter().enumerate() {
        let error = Values::from_query_with(
            &input,
            QueryLimits {
                max_pairs: maximum,
                ..QueryLimits::UNLIMITED
            },
        )
        .unwrap_err();
        assert_eq!(
            (error.kind(), error.pair_offset(), error.parameter()),
            (ErrorKind::TooManyPairs, Some(offset), None)
        );
        assert_eq!(
            error.to_string(),
            format!("query parsing failed at byte {offset}: {}", ErrorKind::TooManyPairs)
        );
        offset += pair.len() + 1;
    }
}

#[test]
fn gamma_round2_scalar_encoded_key_preserves_borrowed_unencoded_value() {
    let input = String::from("it%65m=borrowed");
    let parsed = Borrowed::from_query(&input).expect("encoding only the key does not require an owned value");
    assert_eq!(parsed.item, "borrowed");
    assert_eq!(parsed.item.as_ptr(), input.as_ptr().wrapping_add(7));
}

#[test]
fn gamma_round2_wide_pair_errors_report_absolute_key_and_value_offsets() {
    let suffix = format!("&tail={}", "t".repeat(80));
    for (pair, relative, kind) in [
        ("item=%", 5, ErrorKind::InvalidEncoding),
        ("item=%G0", 5, ErrorKind::InvalidEncoding),
        ("%G0=x", 0, ErrorKind::InvalidEncoding),
        ("item=%FF", 5, ErrorKind::InvalidUtf8),
    ] {
        let input = format!("lead=first&{pair}{suffix}");
        let error = Values::from_query(&input).unwrap_err();
        assert_eq!(error.kind(), kind);
        assert_eq!(error.pair_offset(), Some(11 + relative), "{input}");
    }
}

#[test]
fn gamma_round2_wide_decode_budget_is_cumulative_and_inclusive() {
    for value in ["abc", "%41%42%43"] {
        let input = format!("lead=x&item={value}&tail={}", "z".repeat(80));
        let limits = QueryLimits {
            max_decoded_length: 96,
            ..QueryLimits::UNLIMITED
        };
        let parsed = Values::from_query_with(&input, limits).unwrap();
        assert_eq!(
            (parsed.lead.as_str(), parsed.item.as_str(), parsed.tail.as_str()),
            ("x", if value == "abc" { "abc" } else { "ABC" }, "z".repeat(80).as_str())
        );
        for budget in [5, 6, 8, 9] {
            let error = Values::from_query_with(
                &input,
                QueryLimits {
                    max_decoded_length: budget,
                    ..limits
                },
            )
            .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DecodedTooLong);
            assert_eq!(error.pair_offset(), Some(if budget <= 8 { 7 } else { 12 }));
        }
    }
}
