// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared focused dynamic-router scenarios for Criterion and Callgrind.

use std::hint::black_box;
use std::fmt::Write as _;

use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route};
use routerama::HttpMethod;

#[::routerama::resolver]
#[derive(Debug)]
enum DynamicTypedScenario {
    Unit,
    Parse { value: u32 },
    Owned { value: String },
}

fn route_with_method(name: impl Into<String>, method: &str, path: &str) -> Route {
    Route::new(
        name,
        method,
        PathTemplate::parse(path, Grammar::default().with_segment_affixes()).expect("benchmark route is valid"),
    )
}

fn route(name: impl Into<String>, path: &str) -> Route {
    route_with_method(name, "GET", path)
}

fn build_dynamic_fanout(width: usize) -> (RawResolver, String) {
    let routes = (0..width).map(|index| {
        let path = format!("/items/{index:02}");
        route(format!("Item{index:02}"), &path)
    });
    (RawResolver::new(routes), format!("/items/{:02}", width - 1))
}

fn dynamic_fanout_lookup(scenario: &(RawResolver, String)) {
    black_box(scenario.0.resolve("GET", black_box(&scenario.1)));
}

fn build_dynamic_typed() -> DynamicTypedScenarioResolver {
    DynamicTypedScenario::builder()
        .add_unit(HttpMethod::GET, "/unit")
        .add_parse(HttpMethod::GET, "/parse/{value}")
        .add_owned(HttpMethod::GET, "/owned/{value}")
        .build()
        .expect("typed dynamic scenario builds")
}

#[inline]
fn dynamic_typed_unit(router: &DynamicTypedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/unit")));
}

#[inline]
fn dynamic_typed_parse(router: &DynamicTypedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/parse/12345")));
}

#[inline]
fn dynamic_typed_owned_plain(router: &DynamicTypedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/owned/rust")));
}

#[inline]
fn dynamic_typed_owned_percent(router: &DynamicTypedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/owned/%72ust")));
}

fn build_capture_threshold(captures: usize) -> (RawResolver, String) {
    let mut template = String::from("/captures");
    let mut path = String::from("/captures");
    for index in 0..captures {
        let _ = write!(template, "/{{value{index}}}");
        let _ = write!(path, "/segment{index}");
    }
    (RawResolver::new([route("Captures", &template)]), path)
}

fn dynamic_capture_threshold_lookup(scenario: &(RawResolver, String)) {
    black_box(scenario.0.resolve("GET", black_box(&scenario.1)));
}

fn build_dynamic_misses() -> RawResolver {
    RawResolver::new([
        route("Deep", "/a/b/c/d/e/f/g/h"),
        route_with_method("Submit", "POST", "/submit"),
    ])
}

#[inline]
fn dynamic_early_miss(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/missing")));
}

#[inline]
fn dynamic_late_miss(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/a/b/c/d/e/f/g/missing")));
}

#[inline]
fn dynamic_wrong_method(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/submit")));
}

fn build_dynamic_features() -> RawResolver {
    RawResolver::new([
        route("Rest", "/files/{path=**}"),
        route("Affix", "/img-{id}.png"),
    ])
}

#[inline]
fn dynamic_rest(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/files/a/b/c")));
}

#[inline]
fn dynamic_affix(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/img-cat.png")));
}

fn build_dynamic_no_verb() -> RawResolver {
    RawResolver::new([route("Get", "/books/{book}")])
}

fn build_dynamic_with_verb() -> RawResolver {
    RawResolver::new([
        route("Get", "/books/{book}"),
        route("Archive", "/books/{book}:archive"),
    ])
}

#[inline]
fn dynamic_no_verb(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/books/rust")));
}

#[inline]
fn dynamic_with_verb_nonverb_hit(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/books/rust")));
}

#[inline]
fn dynamic_verb_hit(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/books/rust:archive")));
}

fn build_dynamic_depth(depth: usize) -> (RawResolver, String) {
    let mut deep = String::new();
    for index in 0..depth {
        let _ = write!(deep, "/{index:02}");
    }
    (
        RawResolver::new([route("Hot", "/hot"), route("Deep", &deep)]),
        deep,
    )
}

fn dynamic_depth_table_shallow_lookup(scenario: &(RawResolver, String)) {
    black_box(scenario.0.resolve("GET", black_box("/hot")));
}

fn dynamic_depth_table_deep_lookup(scenario: &(RawResolver, String)) {
    black_box(scenario.0.resolve("GET", black_box(&scenario.1)));
}

fn build_deep_dynamic() -> RawResolver {
    RawResolver::new([
        route("Hot", "/hot"),
        route(
            "Deep",
            "/deep/01/02/03/04/05/06/07/08/09/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31",
        ),
    ])
}

fn dynamic_deep_table_shallow_lookup(router: &RawResolver) {
    black_box(router.resolve("GET", black_box("/hot")));
}

fn dynamic_deep_table_deep_lookup(router: &RawResolver) {
    black_box(router.resolve(
        "GET",
        black_box("/deep/01/02/03/04/05/06/07/08/09/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31"),
    ));
}
