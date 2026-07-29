// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::hint::black_box;

use routerama::query::{FromQuery, ToQuery};
use serde::{Deserialize, Serialize};

const COMMON: &str = "q=rust&page=2&exact=true";
const ESCAPED: &str = "q=rust+language%2Fweb&page=2&exact=true";
const REPEATED: &str = "q=rust&tag=fast&tag=safe&tag=zero+alloc";
const LONG_VALUE: &str = concat!(
    "payload=",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
);

#[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
struct DirectCommon<'q> {
    q: Cow<'q, str>,
    page: u32,
    exact: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct SerdeCommon<'q> {
    #[serde(borrow)]
    q: Cow<'q, str>,
    page: u32,
    exact: bool,
}

#[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
struct DirectRepeated<'q> {
    q: &'q str,
    tag: Vec<Cow<'q, str>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SerdeRepeated {
    q: String,
    tag: Vec<String>,
}

#[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
struct DirectLong<'q> {
    payload: &'q str,
}

#[derive(Debug, Deserialize, Serialize)]
struct SerdeLong<'q> {
    payload: &'q str,
}

fn direct_parse_common() {
    black_box(DirectCommon::from_query(black_box(COMMON)).expect("valid query"));
}

fn serde_urlencoded_parse_common() {
    black_box(serde_urlencoded::from_str::<SerdeCommon<'_>>(black_box(COMMON)).expect("valid query"));
}

fn serde_html_form_parse_common() {
    black_box(serde_html_form::from_str::<SerdeCommon<'_>>(black_box(COMMON)).expect("valid query"));
}

fn direct_parse_escaped() {
    black_box(DirectCommon::from_query(black_box(ESCAPED)).expect("valid query"));
}

fn serde_urlencoded_parse_escaped() {
    black_box(serde_urlencoded::from_str::<SerdeCommon<'_>>(black_box(ESCAPED)).expect("valid query"));
}

fn serde_html_form_parse_escaped() {
    black_box(serde_html_form::from_str::<SerdeCommon<'_>>(black_box(ESCAPED)).expect("valid query"));
}

fn direct_parse_repeated() {
    black_box(DirectRepeated::from_query(black_box(REPEATED)).expect("valid query"));
}

fn serde_html_form_parse_repeated() {
    black_box(serde_html_form::from_str::<SerdeRepeated>(black_box(REPEATED)).expect("valid query"));
}

fn direct_parse_long() {
    black_box(DirectLong::from_query(black_box(LONG_VALUE)).expect("valid query"));
}

fn serde_urlencoded_parse_long() {
    black_box(serde_urlencoded::from_str::<SerdeLong<'_>>(black_box(LONG_VALUE)).expect("valid query"));
}

fn serde_html_form_parse_long() {
    black_box(serde_html_form::from_str::<SerdeLong<'_>>(black_box(LONG_VALUE)).expect("valid query"));
}

fn direct_produce_common(query: &DirectCommon<'_>, output: &mut String) {
    output.clear();
    query.write_query(black_box(output)).expect("query production succeeds");
    black_box(output);
}

fn direct_produce_common_allocating(query: &DirectCommon<'_>) {
    black_box(query.to_query_string().expect("query production succeeds"));
}

fn serde_urlencoded_produce_common(query: &SerdeCommon<'_>) {
    black_box(serde_urlencoded::to_string(black_box(query)).expect("query production succeeds"));
}

fn serde_html_form_produce_common(query: &SerdeCommon<'_>) {
    black_box(serde_html_form::to_string(black_box(query)).expect("query production succeeds"));
}

fn serde_html_form_produce_common_reserved(query: &SerdeCommon<'_>, output: &mut String) {
    output.clear();
    serde_html_form::push_to_string(black_box(output), black_box(query)).expect("query production succeeds");
    black_box(output);
}

fn direct_common_value() -> DirectCommon<'static> {
    DirectCommon {
        q: Cow::Borrowed("rust"),
        page: 2,
        exact: true,
    }
}

fn serde_common_value() -> SerdeCommon<'static> {
    SerdeCommon {
        q: Cow::Borrowed("rust"),
        page: 2,
        exact: true,
    }
}
