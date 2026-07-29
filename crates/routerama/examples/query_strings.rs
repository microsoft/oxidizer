// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed query-string parsing and production.
//!
//! Run with `cargo run --example query_strings`.

use std::borrow::Cow;

use routerama::query::{FromQuery, ToQuery};

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(deny_unknown_fields)]
struct SearchQuery<'q> {
    q: Cow<'q, str>,
    page: Option<u32>,
    tag: Vec<Cow<'q, str>>,
}

fn main() {
    let query = SearchQuery::from_query("q=rust+language&page=2&tag=fast&tag=safe").expect("valid query");
    assert_eq!(query.q, "rust language");
    assert_eq!(query.page, Some(2));
    assert_eq!(query.tag, ["fast", "safe"]);

    assert_eq!(
        query.to_query_string().expect("query production succeeds"),
        "q=rust+language&page=2&tag=fast&tag=safe"
    );
}
