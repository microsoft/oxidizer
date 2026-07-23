// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Round-trip contracts for [`QueryLimits::DEFAULT`].

use routerama::query::{ErrorKind, FromQuery, QueryLimits, ToQuery};

#[derive(Debug, PartialEq, Eq, routerama::query::FromQuery, routerama::query::ToQuery)]
struct Document {
    body: String,
}

#[derive(Debug, PartialEq, Eq, routerama::query::FromQuery, routerama::query::ToQuery)]
struct Tags {
    tag: Vec<String>,
}

#[test]
fn a_query_produced_under_the_default_limits_can_always_be_parsed_back() {
    for length in [0, 1, 100, 8_000, 16_000, 16_378, 16_379, 20_000, 70_000] {
        let document = Document { body: "a".repeat(length) };
        if let Ok(query) = document.to_query_string() {
            let parsed = Document::from_query(&query).expect("a query produced under the default limits parses back");
            assert_eq!(parsed, document);
        }
    }

    for count in [0, 1, 100, 255, 256, 257, 300] {
        let tags = Tags {
            tag: (0..count).map(|index| index.to_string()).collect(),
        };
        if let Ok(query) = tags.to_query_string() {
            let parsed = Tags::from_query(&query).expect("a query produced under the default limits parses back");
            assert_eq!(parsed, tags);
        }
    }
}

#[test]
fn a_value_too_long_for_the_default_parse_limit_is_refused_while_it_is_produced() {
    let document = Document { body: "a".repeat(20_000) };
    let error = document
        .to_query_string()
        .expect_err("a query longer than the default parse limit is refused at production time");
    assert_eq!(error.parameter(), Some("body"));
    assert_eq!(error.pair_offset(), None);
    assert_eq!(error.kind(), ErrorKind::TooLong);
}

#[test]
fn a_value_with_more_pairs_than_the_default_pair_limit_is_refused_while_it_is_produced() {
    let tags = Tags {
        tag: (0..300).map(|index| index.to_string()).collect(),
    };
    let error = tags
        .to_query_string()
        .expect_err("a query with more pairs than the default parse limit is refused at production time");
    assert_eq!(error.parameter(), Some("tag"));
    assert_eq!(error.pair_offset(), None);
    assert_eq!(error.kind(), ErrorKind::TooManyPairs);
}

#[test]
fn a_payload_that_fits_the_default_limits_round_trips_unchanged() {
    let document = Document { body: "a".repeat(1_000) };
    let query = document.to_query_string().expect("query production succeeds");
    assert_eq!(query.len(), 1_005);
    let parsed = Document::from_query(&query).expect("a small query round-trips");
    assert_eq!(parsed, document);

    let tags = Tags {
        tag: (0..256).map(|index| index.to_string()).collect(),
    };
    let query = tags.to_query_string_with(QueryLimits::DEFAULT).expect("query production succeeds");
    let parsed = Tags::from_query_with(&query, QueryLimits::DEFAULT).expect("a query at the default pair limit round-trips");
    assert_eq!(parsed, tags);
}

#[test]
fn limits_other_than_the_default_carry_no_round_trip_guarantee() {
    let tags = Tags {
        tag: (0..300).map(|index| index.to_string()).collect(),
    };
    let query = tags
        .to_query_string_with(QueryLimits::UNLIMITED)
        .expect("unlimited production accepts any pair count");
    assert_eq!(
        Tags::from_query_with(&query, QueryLimits::DEFAULT)
            .expect_err("only the default limits guarantee a round trip")
            .kind(),
        ErrorKind::TooManyPairs
    );
}
