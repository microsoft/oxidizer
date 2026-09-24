// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated benchmark suite for router comparison, router scenarios, and query codecs.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(
    dead_code,
    reason = "the shared harness and scenarios support multiple benchmark groups and benchmark-generated fields are pattern-matched indirectly"
)]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun benchmark inputs are passed and returned by value by the framework"
)]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion (emitted for every platform; Gungraun itself only runs on Linux). Upstream tracking issues are pending."
)]

#[cfg(feature = "query")]
use std::borrow::Cow;
use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
use http_path_template::{Grammar, PathTemplate};
use routerama::__rt::{RawResolver, Route};
use routerama::HttpMethod;
#[cfg(feature = "query")]
use routerama::query::{FromQuery, ToQuery};
#[cfg(feature = "query")]
use serde::{Deserialize, Serialize};

include!("common/routes_data.rs");
include!("common/bench_router.rs");

type BenchRouteRouter = BenchRouteResolver;
type BenchDynRouteRouter = BenchDynRouteResolver;

/// The HTTP method every benchmark route is registered under and every lookup
/// requests. routerama matches it as part of `resolve`; the other routers are
/// path-only, so the harness validates it after matching.
const REQUEST_METHOD: &str = "GET";

/// Paths that share substantial prefixes with the route table but do not match.
static MISS_LOOKUPS: &[&str] = &[
    "/missing",
    "/v1/missing",
    "/v1/users/octocat/missing",
    "/v1/repos/rust-lang/cargo/issues/1347/comments/42/missing",
    "/v1/search/missing",
];

/// Route metadata used to reproduce routerama's typed match result.
type RouteValue = (&'static str, &'static str, &'static [Ty]);

/// Coerces one capture and prevents the result from being optimized away.
#[inline]
fn coerce(value: &str, ty: Ty) {
    match ty {
        Ty::Str => {
            black_box(value);
        }
        Ty::U32 => {
            let _ = black_box(::routerama::__rt::coerce_parse::<u32>(value, "benchmark"));
        }
        Ty::Owned => {
            let _ = black_box(::routerama::__rt::coerce_owned(value, "benchmark"));
        }
    }
}

/// Drives a matched non-routerama route to the typed end state: validate the
/// verb, then coerce each captured value (in template order) to its target type.
#[inline]
fn consume_typed<'a>(
    name: &str,
    registered_method: &str,
    requested_method: &str,
    params: impl Iterator<Item = (&'a str, &'a str)>,
    tys: &[Ty],
) {
    debug_assert_eq!(registered_method, requested_method, "unexpected route method");
    black_box(name);
    black_box(registered_method == requested_method);
    for ((_, value), ty) in params.zip(tys.iter().copied()) {
        coerce(value, ty);
    }
}

/// Makes the request method opaque to the optimizer for every router.
#[inline]
fn request_method() -> &'static str {
    black_box(REQUEST_METHOD)
}

/// The route-parameter syntax a given router expects.
#[derive(Clone, Copy)]
enum Style {
    /// `{p0}` bracketed parameters (matchit).
    Brackets,
    /// `:p0` colon parameters (path-tree, route-recognizer).
    Colon,
    /// A full anchored regular expression (regex `RegexSet`).
    Regex,
}

/// Rewrites a `{var}` route template into the `style` a given router expects,
/// numbering parameters positionally so no route carries a duplicate name.
fn to_pattern(template: &str, style: Style) -> String {
    let mut out = String::with_capacity(template.len() + 2);
    if matches!(style, Style::Regex) {
        out.push('^');
    }
    let mut param = 0_u32;
    for segment in template.split('/') {
        if segment.is_empty() {
            continue;
        }
        out.push('/');
        if segment.starts_with('{') && segment.ends_with('}') {
            match style {
                Style::Brackets => {
                    out.push_str("{p");
                    out.push_str(&param.to_string());
                    out.push('}');
                }
                Style::Colon => {
                    out.push_str(":p");
                    out.push_str(&param.to_string());
                }
                Style::Regex => out.push_str("([^/]+)"),
            }
            param += 1;
        } else {
            out.push_str(segment);
        }
    }
    if matches!(style, Style::Regex) {
        out.push('$');
    }
    out
}

// Static routerama.

fn build_routerama_static() -> BenchRouteRouter {
    BenchRoute::resolver()
}

fn routerama_static_lookups(router: &BenchRouteRouter) {
    for path in LOOKUPS {
        let matched = router.resolve(request_method(), black_box(path));
        debug_assert!(matched.is_ok(), "routerama_static misses {path}");
        let _ = black_box(matched);
    }
}

fn routerama_static_misses(router: &BenchRouteRouter) {
    for path in MISS_LOOKUPS {
        let _ = black_box(router.resolve(request_method(), black_box(path)));
    }
}

// Dynamic routerama.

fn build_routerama_dynamic() -> BenchDynRouteRouter {
    build_bench_dyn()
}

fn routerama_dynamic_lookups(router: &BenchDynRouteRouter) {
    for path in LOOKUPS {
        let matched = router.resolve(request_method(), black_box(path));
        debug_assert!(matched.is_ok(), "routerama_dynamic misses {path}");
        let _ = black_box(matched);
    }
}

fn routerama_dynamic_misses(router: &BenchDynRouteRouter) {
    for path in MISS_LOOKUPS {
        let _ = black_box(router.resolve(request_method(), black_box(path)));
    }
}

// matchit.

fn build_matchit() -> ::matchit::Router<RouteValue> {
    let mut router = ::matchit::Router::new();
    for (name, template, tys) in ROUTES {
        router
            .insert(to_pattern(template, Style::Brackets), (*name, REQUEST_METHOD, *tys))
            .expect("matchit insert");
    }
    router
}

fn matchit_lookups(router: &::matchit::Router<RouteValue>) {
    for path in LOOKUPS {
        let matched = router.at(black_box(path));
        debug_assert!(matched.is_ok(), "matchit misses {path}");
        if let Ok(found) = matched {
            let (name, method, tys) = *found.value;
            consume_typed(name, method, request_method(), found.params.iter(), tys);
        }
    }
}

fn matchit_misses(router: &::matchit::Router<RouteValue>) {
    for path in MISS_LOOKUPS {
        let _ = black_box(router.at(black_box(path)));
    }
}

// path-tree.

fn build_path_tree() -> ::path_tree::PathTree<RouteValue> {
    let mut tree = ::path_tree::PathTree::new();
    for (name, template, tys) in ROUTES {
        let _ = tree.insert(&to_pattern(template, Style::Colon), (*name, REQUEST_METHOD, *tys));
    }
    tree
}

fn path_tree_lookups(tree: &::path_tree::PathTree<RouteValue>) {
    for path in LOOKUPS {
        let matched = tree.find(black_box(path));
        debug_assert!(matched.is_some(), "path-tree misses {path}");
        if let Some((value, matched_path)) = matched {
            let (name, method, tys) = *value;
            consume_typed(name, method, request_method(), matched_path.params_iter(), tys);
        }
    }
}

fn path_tree_misses(tree: &::path_tree::PathTree<RouteValue>) {
    for path in MISS_LOOKUPS {
        black_box(tree.find(black_box(path)));
    }
}

// regex.

/// A regex "router": a `RegexSet` for winner selection plus the per-route
/// `Regex`, name, method, and capture types used to validate and coerce.
struct RegexRouter {
    set: ::regex::RegexSet,
    regexes: Vec<::regex::Regex>,
    routes: Vec<RouteValue>,
}

fn build_regex() -> RegexRouter {
    let patterns: Vec<String> = ROUTES.iter().map(|(_, template, _)| to_pattern(template, Style::Regex)).collect();
    let set = ::regex::RegexSet::new(&patterns).expect("regex set");
    let regexes = patterns
        .iter()
        .map(|pattern| ::regex::Regex::new(pattern).expect("regex"))
        .collect();
    let routes = ROUTES.iter().map(|(name, _, tys)| (*name, REQUEST_METHOD, *tys)).collect();
    RegexRouter { set, regexes, routes }
}

fn regex_lookups(router: &RegexRouter) {
    for path in LOOKUPS {
        let matches = router.set.matches(black_box(path));
        debug_assert!(matches.matched_any(), "regex misses {path}");
        if let Some(index) = matches.iter().next() {
            let (name, method, tys) = router.routes[index];
            let requested_method = request_method();
            debug_assert_eq!(method, requested_method, "unexpected route method");
            black_box(name);
            black_box(method == requested_method);
            if let Some(captures) = router.regexes[index].captures(path) {
                // Group 0 is the whole match; groups 1.. are the variables, in
                // positional (template) order, so they align with `tys`.
                for (group, ty) in captures.iter().skip(1).flatten().zip(tys.iter().copied()) {
                    coerce(group.as_str(), ty);
                }
            }
        }
    }
}

fn regex_misses(router: &RegexRouter) {
    for path in MISS_LOOKUPS {
        black_box(router.set.matches(black_box(path)));
    }
}

// route-recognizer.

fn build_route_recognizer() -> ::route_recognizer::Router<RouteValue> {
    let mut router = ::route_recognizer::Router::new();
    for (name, template, tys) in ROUTES {
        router.add(&to_pattern(template, Style::Colon), (*name, REQUEST_METHOD, *tys));
    }
    router
}

fn route_recognizer_lookups(router: &::route_recognizer::Router<RouteValue>) {
    for path in LOOKUPS {
        let matched = router.recognize(black_box(path));
        debug_assert!(matched.is_ok(), "route-recognizer misses {path}");
        if let Ok(found) = matched {
            let (name, method, tys) = **found.handler();
            consume_typed(name, method, request_method(), found.params().iter(), tys);
        }
    }
}

fn route_recognizer_misses(router: &::route_recognizer::Router<RouteValue>) {
    for path in MISS_LOOKUPS {
        let _ = black_box(router.recognize(black_box(path)));
    }
}

// Setup helpers warm each router before measurement.

fn build_hot_routerama_static() -> BenchRouteRouter {
    let router = build_routerama_static();
    routerama_static_lookups(&router);
    router
}

fn build_hot_routerama_dynamic() -> BenchDynRouteRouter {
    let router = build_routerama_dynamic();
    routerama_dynamic_lookups(&router);
    router
}

fn build_hot_matchit() -> ::matchit::Router<RouteValue> {
    let router = build_matchit();
    matchit_lookups(&router);
    router
}

fn build_hot_path_tree() -> ::path_tree::PathTree<RouteValue> {
    let router = build_path_tree();
    path_tree_lookups(&router);
    router
}

fn build_hot_regex() -> RegexRouter {
    let router = build_regex();
    regex_lookups(&router);
    router
}

fn build_hot_route_recognizer() -> ::route_recognizer::Router<RouteValue> {
    let router = build_route_recognizer();
    route_recognizer_lookups(&router);
    router
}

#[::routerama::resolver]
#[derive(Debug)]
enum StaticScenario<'p> {
    #[route(GET, "/health")]
    Health,
    #[route(GET, "/a/b/c/d/e/f/g/h")]
    Deep,
    #[route(GET, "/fanout/00")]
    Fanout00,
    #[route(GET, "/fanout/01")]
    Fanout01,
    #[route(GET, "/fanout/02")]
    Fanout02,
    #[route(GET, "/fanout/03")]
    Fanout03,
    #[route(GET, "/fanout/04")]
    Fanout04,
    #[route(GET, "/fanout/05")]
    Fanout05,
    #[route(GET, "/fanout/06")]
    Fanout06,
    #[route(GET, "/fanout/07")]
    Fanout07,
    #[route(GET, "/fanout/08")]
    Fanout08,
    #[route(GET, "/fanout/09")]
    Fanout09,
    #[route(GET, "/fanout/10")]
    Fanout10,
    #[route(GET, "/fanout/11")]
    Fanout11,
    #[route(GET, "/fanout/12")]
    Fanout12,
    #[route(GET, "/fanout/13")]
    Fanout13,
    #[route(GET, "/fanout/14")]
    Fanout14,
    #[route(GET, "/fanout/15")]
    Fanout15,
    #[route(GET, "/users/{user}")]
    BorrowOne { user: &'p str },
    #[route(GET, "/orgs/{org}/repos/{repo}/refs/{kind}/{name}")]
    BorrowFour {
        org: &'p str,
        repo: &'p str,
        kind: &'p str,
        name: &'p str,
    },
    #[route(GET, "/issues/{issue}")]
    ParseNumber { issue: u32 },
    #[route(GET, "/owned/{name}")]
    OwnString { name: String },
    #[route(POST, "/submit")]
    Submit,
    #[route(GET, "/files/{path=**}")]
    Files { path: &'p str },
    #[route(GET, "/img-{id}.png")]
    Image { id: &'p str },
}

#[::routerama::resolver]
#[derive(Debug)]
enum NoVerbScenario<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
}

#[::routerama::resolver]
#[derive(Debug)]
enum WithVerbScenario<'p> {
    #[route(GET, "/books/{book}")]
    GetBook { book: &'p str },
    #[route(GET, "/books/{book}:archive")]
    ArchiveBook { book: &'p str },
}

#[::routerama::resolver]
#[derive(Debug)]
enum ShallowTable {
    #[route(GET, "/hot")]
    Hot,
}

#[::routerama::resolver]
#[derive(Debug)]
enum DeepOutlierTable {
    #[route(GET, "/hot")]
    Hot,
    #[route(
        GET,
        "/deep/01/02/03/04/05/06/07/08/09/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31"
    )]
    Deep,
}

#[::routerama::resolver]
#[derive(Debug)]
enum AffixFanout<'p> {
    #[route(GET, "/asset-00-{id}.png")]
    Asset00 { id: &'p str },
    #[route(GET, "/asset-01-{id}.png")]
    Asset01 { id: &'p str },
    #[route(GET, "/asset-02-{id}.png")]
    Asset02 { id: &'p str },
    #[route(GET, "/asset-03-{id}.png")]
    Asset03 { id: &'p str },
    #[route(GET, "/asset-04-{id}.png")]
    Asset04 { id: &'p str },
    #[route(GET, "/asset-05-{id}.png")]
    Asset05 { id: &'p str },
    #[route(GET, "/asset-06-{id}.png")]
    Asset06 { id: &'p str },
    #[route(GET, "/asset-07-{id}.png")]
    Asset07 { id: &'p str },
    #[route(GET, "/asset-08-{id}.png")]
    Asset08 { id: &'p str },
    #[route(GET, "/asset-09-{id}.png")]
    Asset09 { id: &'p str },
    #[route(GET, "/asset-10-{id}.png")]
    Asset10 { id: &'p str },
    #[route(GET, "/asset-11-{id}.png")]
    Asset11 { id: &'p str },
    #[route(GET, "/asset-12-{id}.png")]
    Asset12 { id: &'p str },
    #[route(GET, "/asset-13-{id}.png")]
    Asset13 { id: &'p str },
    #[route(GET, "/asset-14-{id}.png")]
    Asset14 { id: &'p str },
    #[route(GET, "/asset-15-{id}.png")]
    Asset15 { id: &'p str },
}

type StaticScenarioRouter = StaticScenarioResolver;
type NoVerbScenarioRouter = NoVerbScenarioResolver;
type WithVerbScenarioRouter = WithVerbScenarioResolver;
type ShallowTableRouter = ShallowTableResolver;
type DeepOutlierTableRouter = DeepOutlierTableResolver;
type AffixFanoutRouter = AffixFanoutResolver;

fn build_static_scenario() -> StaticScenarioRouter {
    StaticScenario::resolver()
}

fn build_no_verb_scenario() -> NoVerbScenarioRouter {
    NoVerbScenario::resolver()
}

fn build_with_verb_scenario() -> WithVerbScenarioRouter {
    WithVerbScenario::resolver()
}

fn build_shallow_table() -> ShallowTableRouter {
    ShallowTable::resolver()
}

fn build_deep_outlier_table() -> DeepOutlierTableRouter {
    DeepOutlierTable::resolver()
}

fn build_affix_fanout() -> AffixFanoutRouter {
    AffixFanout::resolver()
}

#[inline]
fn static_shallow_literal(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/health")));
}

#[inline]
fn static_deep_literal(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/a/b/c/d/e/f/g/h")));
}

#[inline]
fn static_fanout_first(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/fanout/00")));
}

#[inline]
fn static_fanout_middle(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/fanout/08")));
}

#[inline]
fn static_fanout_last(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/fanout/15")));
}

#[inline]
fn static_borrow_one(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/users/alice")));
}

#[inline]
fn static_borrow_four(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/orgs/acme/repos/api/refs/heads/main")));
}

#[inline]
fn static_parse_number(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/issues/12345")));
}

#[inline]
fn static_own_plain(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/owned/rust")));
}

#[inline]
fn static_own_percent(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/owned/%72ust")));
}

#[inline]
fn static_early_miss(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/missing")));
}

#[inline]
fn static_late_miss(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/a/b/c/d/e/f/g/missing")));
}

#[inline]
fn static_pathological_long_miss(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve(
        "GET",
        black_box(
            "/00/01/02/03/04/05/06/07/08/09/10/11/12/13/14/15/16/17/18/19/20/21/22/23/24/25/26/27/28/29/30/31/32/33/34/35/36/37/38/39/40/41/42/43/44/45/46/47/48/49/50/51/52/53/54/55/56/57/58/59/60/61/62/63",
        ),
    ));
}

#[inline]
fn static_wrong_method(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/submit")));
}

#[inline]
fn static_rest(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/files/a/b/c")));
}

#[inline]
fn static_affix(router: &StaticScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/img-cat.png")));
}

#[inline]
fn static_no_verb(router: &NoVerbScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/books/rust")));
}

#[inline]
fn static_with_verb_nonverb_hit(router: &WithVerbScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/books/rust")));
}

#[inline]
fn static_with_verb_hit(router: &WithVerbScenarioRouter) {
    let _ = black_box(router.resolve("GET", black_box("/books/rust:archive")));
}

#[inline]
fn static_shallow_table_hit(router: &ShallowTableRouter) {
    let _ = black_box(router.resolve("GET", black_box("/hot")));
}

#[inline]
fn static_deep_outlier_table_hit(router: &DeepOutlierTableRouter) {
    let _ = black_box(router.resolve("GET", black_box("/hot")));
}

#[inline]
fn static_affix_fanout_first(router: &AffixFanoutRouter) {
    let _ = black_box(router.resolve("GET", black_box("/asset-00-cat.png")));
}

#[inline]
fn static_affix_fanout_middle(router: &AffixFanoutRouter) {
    let _ = black_box(router.resolve("GET", black_box("/asset-08-cat.png")));
}

#[inline]
fn static_affix_fanout_last(router: &AffixFanoutRouter) {
    let _ = black_box(router.resolve("GET", black_box("/asset-15-cat.png")));
}

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
    RawResolver::new([route("Deep", "/a/b/c/d/e/f/g/h"), route_with_method("Submit", "POST", "/submit")])
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
    RawResolver::new([route("Rest", "/files/{path=**}"), route("Affix", "/img-{id}.png")])
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
    RawResolver::new([route("Get", "/books/{book}"), route("Archive", "/books/{book}:archive")])
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
    (RawResolver::new([route("Hot", "/hot"), route("Deep", &deep)]), deep)
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

#[::routerama::resolver]
#[derive(Debug)]
enum MixedScenario {
    #[route(GET, "/health")]
    Health,
    #[route(GET, "/numbers/{value}")]
    Number {
        value: u32,
    },
    Plugin {
        name: String,
    },
    NumberFallback {
        value: String,
    },
}

fn build_mixed_scenario() -> MixedScenarioResolver {
    MixedScenario::builder()
        .add_plugin(HttpMethod::GET, "/plugins/{name}")
        .add_number_fallback(HttpMethod::GET, "/numbers/{value}")
        .build()
        .expect("mixed scenario builds")
}

#[inline]
fn mixed_static_hit(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/health")));
}

#[inline]
fn mixed_dynamic_hit(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/plugins/auth")));
}

#[inline]
fn mixed_complete_miss(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/missing")));
}

#[inline]
fn mixed_static_capture_error(router: &MixedScenarioResolver) {
    let _ = black_box(router.resolve("GET", black_box("/numbers/not-a-number")));
}

#[cfg(feature = "query")]
mod query_support {
    use super::*;

    pub(crate) const COMMON: &str = "q=rust&page=2&exact=true";
    pub(crate) const ESCAPED: &str = "q=rust+language%2Fweb&page=2&exact=true";
    pub(crate) const REPEATED: &str = "q=rust&tag=fast&tag=safe&tag=zero+alloc";
    pub(crate) const LONG_VALUE: &str = concat!(
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
    pub(crate) struct DirectCommon<'q> {
        q: Cow<'q, str>,
        page: u32,
        exact: bool,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub(crate) struct SerdeCommon<'q> {
        #[serde(borrow)]
        q: Cow<'q, str>,
        page: u32,
        exact: bool,
    }

    #[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
    pub(crate) struct DirectRepeated<'q> {
        q: &'q str,
        tag: Vec<Cow<'q, str>>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub(crate) struct SerdeRepeated {
        q: String,
        tag: Vec<String>,
    }

    #[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
    pub(crate) struct DirectLong<'q> {
        payload: &'q str,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub(crate) struct SerdeLong<'q> {
        payload: &'q str,
    }

    pub(crate) fn direct_parse_common() {
        black_box(DirectCommon::from_query(black_box(COMMON)).expect("valid query"));
    }

    pub(crate) fn serde_urlencoded_parse_common() {
        black_box(serde_urlencoded::from_str::<SerdeCommon<'_>>(black_box(COMMON)).expect("valid query"));
    }

    pub(crate) fn serde_html_form_parse_common() {
        black_box(serde_html_form::from_str::<SerdeCommon<'_>>(black_box(COMMON)).expect("valid query"));
    }

    pub(crate) fn direct_parse_escaped() {
        black_box(DirectCommon::from_query(black_box(ESCAPED)).expect("valid query"));
    }

    pub(crate) fn serde_urlencoded_parse_escaped() {
        black_box(serde_urlencoded::from_str::<SerdeCommon<'_>>(black_box(ESCAPED)).expect("valid query"));
    }

    pub(crate) fn serde_html_form_parse_escaped() {
        black_box(serde_html_form::from_str::<SerdeCommon<'_>>(black_box(ESCAPED)).expect("valid query"));
    }

    pub(crate) fn direct_parse_repeated() {
        black_box(DirectRepeated::from_query(black_box(REPEATED)).expect("valid query"));
    }

    pub(crate) fn serde_html_form_parse_repeated() {
        black_box(serde_html_form::from_str::<SerdeRepeated>(black_box(REPEATED)).expect("valid query"));
    }

    pub(crate) fn direct_parse_long() {
        black_box(DirectLong::from_query(black_box(LONG_VALUE)).expect("valid query"));
    }

    pub(crate) fn serde_urlencoded_parse_long() {
        black_box(serde_urlencoded::from_str::<SerdeLong<'_>>(black_box(LONG_VALUE)).expect("valid query"));
    }

    pub(crate) fn serde_html_form_parse_long() {
        black_box(serde_html_form::from_str::<SerdeLong<'_>>(black_box(LONG_VALUE)).expect("valid query"));
    }

    pub(crate) fn direct_produce_common(query: &DirectCommon<'_>, output: &mut String) {
        output.clear();
        query.write_query(black_box(output)).expect("query production succeeds");
        black_box(output);
    }

    pub(crate) fn direct_produce_common_allocating(query: &DirectCommon<'_>) {
        black_box(query.to_query_string().expect("query production succeeds"));
    }

    pub(crate) fn serde_urlencoded_produce_common(query: &SerdeCommon<'_>) {
        black_box(serde_urlencoded::to_string(black_box(query)).expect("query production succeeds"));
    }

    pub(crate) fn serde_html_form_produce_common(query: &SerdeCommon<'_>) {
        black_box(serde_html_form::to_string(black_box(query)).expect("query production succeeds"));
    }

    pub(crate) fn serde_html_form_produce_common_reserved(query: &SerdeCommon<'_>, output: &mut String) {
        output.clear();
        serde_html_form::push_to_string(black_box(output), black_box(query)).expect("query production succeeds");
        black_box(output);
    }

    pub(crate) fn direct_common_value() -> DirectCommon<'static> {
        DirectCommon {
            q: Cow::Borrowed("rust"),
            page: 2,
            exact: true,
        }
    }

    pub(crate) fn serde_common_value() -> SerdeCommon<'static> {
        SerdeCommon {
            q: Cow::Borrowed("rust"),
            page: 2,
            exact: true,
        }
    }
}

#[cfg(feature = "query")]
use query_support::*;

#[expect(
    clippy::similar_names,
    clippy::too_many_lines,
    reason = "the consolidated Criterion registration preserves the original benchmark group layout"
)]
fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("compare_routers");

    let routerama_static = build_hot_routerama_static();
    group.bench_function("routerama_static", |bencher| {
        bencher.iter(|| routerama_static_lookups(&routerama_static));
    });

    {
        let dynamic = build_hot_routerama_dynamic();
        group.bench_function("routerama_dynamic", |bencher| {
            bencher.iter(|| routerama_dynamic_lookups(&dynamic));
        });
    }

    let matchit = build_hot_matchit();
    group.bench_function("matchit", |bencher| bencher.iter(|| matchit_lookups(&matchit)));

    let path_tree = build_hot_path_tree();
    group.bench_function("path_tree", |bencher| bencher.iter(|| path_tree_lookups(&path_tree)));

    let regex_router = build_hot_regex();
    group.bench_function("regex", |bencher| bencher.iter(|| regex_lookups(&regex_router)));

    let route_recognizer = build_hot_route_recognizer();
    group.bench_function("route_recognizer", |bencher| {
        bencher.iter(|| route_recognizer_lookups(&route_recognizer));
    });
    group.finish();

    let mut misses = criterion.benchmark_group("compare_router_misses");
    misses.bench_function("routerama_static", |bencher| {
        bencher.iter(|| routerama_static_misses(&routerama_static));
    });
    {
        let dynamic = build_hot_routerama_dynamic();
        misses.bench_function("routerama_dynamic", |bencher| {
            bencher.iter(|| routerama_dynamic_misses(&dynamic));
        });
    }
    misses.bench_function("matchit", |bencher| bencher.iter(|| matchit_misses(&matchit)));
    misses.bench_function("path_tree", |bencher| bencher.iter(|| path_tree_misses(&path_tree)));
    misses.bench_function("regex", |bencher| bencher.iter(|| regex_misses(&regex_router)));
    misses.bench_function("route_recognizer", |bencher| {
        bencher.iter(|| route_recognizer_misses(&route_recognizer));
    });
    misses.finish();

    let mut construction = criterion.benchmark_group("router_construction");
    construction.sample_size(10);
    construction.bench_function("routerama_dynamic_46_routes", |bencher| {
        bencher.iter_with_large_drop(build_routerama_dynamic);
    });
    construction.bench_function("matchit_46_routes", |bencher| {
        bencher.iter_with_large_drop(build_matchit);
    });
    construction.bench_function("path_tree_46_routes", |bencher| {
        bencher.iter_with_large_drop(build_path_tree);
    });
    construction.bench_function("regex_46_routes", |bencher| {
        bencher.iter_with_large_drop(build_regex);
    });
    construction.bench_function("route_recognizer_46_routes", |bencher| {
        bencher.iter_with_large_drop(build_route_recognizer);
    });
    construction.finish();

    let router = build_static_scenario();
    let mut hits = criterion.benchmark_group("routerama_static/hits");
    hits.bench_function("shallow_literal", |bencher| bencher.iter(|| static_shallow_literal(&router)));
    hits.bench_function("deep_literal", |bencher| bencher.iter(|| static_deep_literal(&router)));
    hits.bench_function("fanout_first", |bencher| bencher.iter(|| static_fanout_first(&router)));
    hits.bench_function("fanout_middle", |bencher| bencher.iter(|| static_fanout_middle(&router)));
    hits.bench_function("fanout_last", |bencher| bencher.iter(|| static_fanout_last(&router)));
    hits.bench_function("borrow_one", |bencher| bencher.iter(|| static_borrow_one(&router)));
    hits.bench_function("borrow_four", |bencher| bencher.iter(|| static_borrow_four(&router)));
    hits.bench_function("parse_number", |bencher| bencher.iter(|| static_parse_number(&router)));
    hits.bench_function("own_plain", |bencher| bencher.iter(|| static_own_plain(&router)));
    hits.bench_function("own_percent", |bencher| bencher.iter(|| static_own_percent(&router)));
    hits.finish();

    let mut static_misses = criterion.benchmark_group("routerama_static/misses");
    static_misses.bench_function("early", |bencher| bencher.iter(|| static_early_miss(&router)));
    static_misses.bench_function("late", |bencher| bencher.iter(|| static_late_miss(&router)));
    static_misses.bench_function("pathological_long", |bencher| {
        bencher.iter(|| static_pathological_long_miss(&router));
    });
    static_misses.bench_function("wrong_method", |bencher| bencher.iter(|| static_wrong_method(&router)));
    static_misses.finish();

    let no_verb = build_no_verb_scenario();
    let with_verb = build_with_verb_scenario();
    let mut features = criterion.benchmark_group("routerama_static/features");
    features.bench_function("rest", |bencher| bencher.iter(|| static_rest(&router)));
    features.bench_function("affix", |bencher| bencher.iter(|| static_affix(&router)));
    features.bench_function("no_verb_table", |bencher| bencher.iter(|| static_no_verb(&no_verb)));
    features.bench_function("verb_table_nonverb_hit", |bencher| {
        bencher.iter(|| static_with_verb_nonverb_hit(&with_verb));
    });
    features.bench_function("verb_hit", |bencher| bencher.iter(|| static_with_verb_hit(&with_verb)));
    features.finish();

    let shallow = build_shallow_table();
    let deep_outlier = build_deep_outlier_table();
    let mut shape = criterion.benchmark_group("routerama_static/table_shape");
    shape.bench_function("shallow_table_hit", |bencher| {
        bencher.iter(|| static_shallow_table_hit(&shallow));
    });
    shape.bench_function("deep_outlier_table_hit", |bencher| {
        bencher.iter(|| static_deep_outlier_table_hit(&deep_outlier));
    });
    let affix_fanout = build_affix_fanout();
    shape.bench_function("affix_fanout_first", |bencher| {
        bencher.iter(|| static_affix_fanout_first(&affix_fanout));
    });
    shape.bench_function("affix_fanout_middle", |bencher| {
        bencher.iter(|| static_affix_fanout_middle(&affix_fanout));
    });
    shape.bench_function("affix_fanout_last", |bencher| {
        bencher.iter(|| static_affix_fanout_last(&affix_fanout));
    });
    shape.finish();

    let typed = build_dynamic_typed();
    let mut typed_group = criterion.benchmark_group("routerama_dynamic/typed");
    typed_group.bench_function("unit", |bencher| bencher.iter(|| dynamic_typed_unit(&typed)));
    typed_group.bench_function("parse", |bencher| bencher.iter(|| dynamic_typed_parse(&typed)));
    typed_group.bench_function("owned_plain", |bencher| bencher.iter(|| dynamic_typed_owned_plain(&typed)));
    typed_group.bench_function("owned_percent", |bencher| bencher.iter(|| dynamic_typed_owned_percent(&typed)));
    typed_group.finish();

    let mut fanout = criterion.benchmark_group("routerama_dynamic/fanout");
    let fanout_1 = build_dynamic_fanout(1);
    fanout.bench_with_input(BenchmarkId::from_parameter(1), &fanout_1, |bencher, scenario| {
        bencher.iter(|| dynamic_fanout_lookup(scenario));
    });
    let fanout_2 = build_dynamic_fanout(2);
    fanout.bench_with_input(BenchmarkId::from_parameter(2), &fanout_2, |bencher, scenario| {
        bencher.iter(|| dynamic_fanout_lookup(scenario));
    });
    let fanout_4 = build_dynamic_fanout(4);
    fanout.bench_with_input(BenchmarkId::from_parameter(4), &fanout_4, |bencher, scenario| {
        bencher.iter(|| dynamic_fanout_lookup(scenario));
    });
    let fanout_8 = build_dynamic_fanout(8);
    fanout.bench_with_input(BenchmarkId::from_parameter(8), &fanout_8, |bencher, scenario| {
        bencher.iter(|| dynamic_fanout_lookup(scenario));
    });
    let fanout_16 = build_dynamic_fanout(16);
    fanout.bench_with_input(BenchmarkId::from_parameter(16), &fanout_16, |bencher, scenario| {
        bencher.iter(|| dynamic_fanout_lookup(scenario));
    });
    let fanout_32 = build_dynamic_fanout(32);
    fanout.bench_with_input(BenchmarkId::from_parameter(32), &fanout_32, |bencher, scenario| {
        bencher.iter(|| dynamic_fanout_lookup(scenario));
    });
    let fanout_64 = build_dynamic_fanout(64);
    fanout.bench_with_input(BenchmarkId::from_parameter(64), &fanout_64, |bencher, scenario| {
        bencher.iter(|| dynamic_fanout_lookup(scenario));
    });
    fanout.finish();

    let mut capture_count = criterion.benchmark_group("routerama_dynamic/capture_count");
    let capture_count_4 = build_capture_threshold(4);
    capture_count.bench_with_input(BenchmarkId::from_parameter(4), &capture_count_4, |bencher, scenario| {
        bencher.iter(|| dynamic_capture_threshold_lookup(scenario));
    });
    let capture_count_5 = build_capture_threshold(5);
    capture_count.bench_with_input(BenchmarkId::from_parameter(5), &capture_count_5, |bencher, scenario| {
        bencher.iter(|| dynamic_capture_threshold_lookup(scenario));
    });
    capture_count.finish();

    let misses_router = build_dynamic_misses();
    let mut dynamic_misses = criterion.benchmark_group("routerama_dynamic/misses");
    dynamic_misses.bench_function("early", |bencher| bencher.iter(|| dynamic_early_miss(&misses_router)));
    dynamic_misses.bench_function("late", |bencher| bencher.iter(|| dynamic_late_miss(&misses_router)));
    dynamic_misses.bench_function("wrong_method", |bencher| {
        bencher.iter(|| dynamic_wrong_method(&misses_router));
    });
    dynamic_misses.finish();

    let features_router = build_dynamic_features();
    let dynamic_no_verb_router = build_dynamic_no_verb();
    let dynamic_with_verb_router = build_dynamic_with_verb();
    let mut dynamic_features = criterion.benchmark_group("routerama_dynamic/features");
    dynamic_features.bench_function("rest", |bencher| bencher.iter(|| dynamic_rest(&features_router)));
    dynamic_features.bench_function("affix", |bencher| bencher.iter(|| dynamic_affix(&features_router)));
    dynamic_features.bench_function("no_verb_table", |bencher| {
        bencher.iter(|| dynamic_no_verb(&dynamic_no_verb_router));
    });
    dynamic_features.bench_function("verb_table_nonverb_hit", |bencher| {
        bencher.iter(|| dynamic_with_verb_nonverb_hit(&dynamic_with_verb_router));
    });
    dynamic_features.bench_function("verb_hit", |bencher| {
        bencher.iter(|| dynamic_verb_hit(&dynamic_with_verb_router));
    });
    dynamic_features.finish();

    let depth_16 = build_dynamic_depth(16);
    let depth_17 = build_dynamic_depth(17);
    let mut segment_depth = criterion.benchmark_group("routerama_dynamic/segment_depth");
    segment_depth.bench_function("shallow_in_16_table", |bencher| {
        bencher.iter(|| dynamic_depth_table_shallow_lookup(&depth_16));
    });
    segment_depth.bench_function("shallow_in_17_table", |bencher| {
        bencher.iter(|| dynamic_depth_table_shallow_lookup(&depth_17));
    });
    segment_depth.bench_function("deep_16", |bencher| {
        bencher.iter(|| dynamic_depth_table_deep_lookup(&depth_16));
    });
    segment_depth.bench_function("deep_17", |bencher| {
        bencher.iter(|| dynamic_depth_table_deep_lookup(&depth_17));
    });
    segment_depth.finish();

    let deep = build_deep_dynamic();
    let mut scratch = criterion.benchmark_group("routerama_dynamic/deep_scratch");
    scratch.bench_function("shallow_lookup", |bencher| {
        bencher.iter(|| dynamic_deep_table_shallow_lookup(&deep));
    });
    scratch.bench_function("deep_lookup", |bencher| {
        bencher.iter(|| dynamic_deep_table_deep_lookup(&deep));
    });
    scratch.finish();

    let mixed = build_mixed_scenario();
    let mut dispatch = criterion.benchmark_group("routerama_mixed/dispatch");
    dispatch.bench_function("static_hit", |bencher| bencher.iter(|| mixed_static_hit(&mixed)));
    dispatch.bench_function("dynamic_fallback_hit", |bencher| {
        bencher.iter(|| mixed_dynamic_hit(&mixed));
    });
    dispatch.bench_function("complete_miss", |bencher| bencher.iter(|| mixed_complete_miss(&mixed)));
    dispatch.bench_function("static_capture_error", |bencher| {
        bencher.iter(|| mixed_static_capture_error(&mixed));
    });
    dispatch.finish();

    #[cfg(feature = "query")]
    query_criterion_benchmarks(criterion);
}

#[cfg(feature = "query")]
fn query_criterion_benchmarks(criterion: &mut Criterion) {
    let mut parsing = criterion.benchmark_group("routerama_query/parse_common");
    parsing.bench_function("routerama", |bencher| bencher.iter(direct_parse_common));
    parsing.bench_function("serde_urlencoded", |bencher| bencher.iter(serde_urlencoded_parse_common));
    parsing.bench_function("serde_html_form", |bencher| bencher.iter(serde_html_form_parse_common));
    parsing.finish();

    let mut escaped = criterion.benchmark_group("routerama_query/parse_escaped");
    escaped.bench_function("routerama", |bencher| bencher.iter(direct_parse_escaped));
    escaped.bench_function("serde_urlencoded", |bencher| bencher.iter(serde_urlencoded_parse_escaped));
    escaped.bench_function("serde_html_form", |bencher| bencher.iter(serde_html_form_parse_escaped));
    escaped.finish();

    let mut repeated = criterion.benchmark_group("routerama_query/parse_repeated");
    repeated.bench_function("routerama", |bencher| bencher.iter(direct_parse_repeated));
    repeated.bench_function("serde_html_form", |bencher| bencher.iter(serde_html_form_parse_repeated));
    repeated.finish();

    let mut long = criterion.benchmark_group("routerama_query/parse_long_ascii");
    long.bench_function("routerama", |bencher| bencher.iter(direct_parse_long));
    long.bench_function("serde_urlencoded", |bencher| bencher.iter(serde_urlencoded_parse_long));
    long.bench_function("serde_html_form", |bencher| bencher.iter(serde_html_form_parse_long));
    long.finish();

    let direct = direct_common_value();
    let serde = serde_common_value();
    let mut output = String::with_capacity(64);
    let mut production = criterion.benchmark_group("routerama_query/produce_common");
    production.bench_function("routerama_reserved", |bencher| {
        bencher.iter(|| direct_produce_common(&direct, &mut output));
    });
    production.bench_function("serde_html_form_reserved", |bencher| {
        bencher.iter(|| serde_html_form_produce_common_reserved(&serde, &mut output));
    });
    production.finish();

    let mut allocating = criterion.benchmark_group("routerama_query/produce_common_allocating");
    allocating.bench_function("routerama", |bencher| {
        bencher.iter(|| direct_produce_common_allocating(&direct));
    });
    allocating.bench_function("serde_urlencoded", |bencher| {
        bencher.iter(|| serde_urlencoded_produce_common(&serde));
    });
    allocating.bench_function("serde_html_form", |bencher| {
        bencher.iter(|| serde_html_form_produce_common(&serde));
    });
    allocating.finish();
}

#[metabench::benchmark(COMPARE_ROUTERS_ROUTERAMA_STATIC, "compare_routers", "routerama_static")]
#[bench::run(build_hot_routerama_static())]
fn routerama_static(router: BenchRouteRouter) -> BenchRouteRouter {
    routerama_static_lookups(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTERS_ROUTERAMA_DYNAMIC, "compare_routers", "routerama_dynamic")]
#[bench::run(build_hot_routerama_dynamic())]
fn routerama_dynamic(router: BenchDynRouteRouter) -> BenchDynRouteRouter {
    routerama_dynamic_lookups(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTERS_MATCHIT, "compare_routers", "matchit")]
#[bench::run(build_hot_matchit())]
fn matchit(router: ::matchit::Router<RouteValue>) -> ::matchit::Router<RouteValue> {
    matchit_lookups(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTERS_PATH_TREE, "compare_routers", "path_tree")]
#[bench::run(build_hot_path_tree())]
fn path_tree(router: ::path_tree::PathTree<RouteValue>) -> ::path_tree::PathTree<RouteValue> {
    path_tree_lookups(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTERS_REGEX, "compare_routers", "regex")]
#[bench::run(build_hot_regex())]
fn regex(router: RegexRouter) -> RegexRouter {
    regex_lookups(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTERS_ROUTE_RECOGNIZER, "compare_routers", "route_recognizer")]
#[bench::run(build_hot_route_recognizer())]
fn route_recognizer(router: ::route_recognizer::Router<RouteValue>) -> ::route_recognizer::Router<RouteValue> {
    route_recognizer_lookups(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTER_MISSES_ROUTERAMA_STATIC, "compare_router_misses", "routerama_static")]
#[bench::run(build_hot_routerama_static())]
fn misses_routerama_static(router: BenchRouteRouter) -> BenchRouteRouter {
    routerama_static_misses(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTER_MISSES_ROUTERAMA_DYNAMIC, "compare_router_misses", "routerama_dynamic")]
#[bench::run(build_hot_routerama_dynamic())]
fn misses_routerama_dynamic(router: BenchDynRouteRouter) -> BenchDynRouteRouter {
    routerama_dynamic_misses(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTER_MISSES_MATCHIT, "compare_router_misses", "matchit")]
#[bench::run(build_hot_matchit())]
fn misses_matchit(router: ::matchit::Router<RouteValue>) -> ::matchit::Router<RouteValue> {
    matchit_misses(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTER_MISSES_PATH_TREE, "compare_router_misses", "path_tree")]
#[bench::run(build_hot_path_tree())]
fn misses_path_tree(router: ::path_tree::PathTree<RouteValue>) -> ::path_tree::PathTree<RouteValue> {
    path_tree_misses(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTER_MISSES_REGEX, "compare_router_misses", "regex")]
#[bench::run(build_hot_regex())]
fn misses_regex(router: RegexRouter) -> RegexRouter {
    regex_misses(&router);
    router
}

#[metabench::benchmark(COMPARE_ROUTER_MISSES_ROUTE_RECOGNIZER, "compare_router_misses", "route_recognizer")]
#[bench::run(build_hot_route_recognizer())]
fn misses_route_recognizer(router: ::route_recognizer::Router<RouteValue>) -> ::route_recognizer::Router<RouteValue> {
    route_recognizer_misses(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_SHALLOW_LITERAL, "routerama_static/hits", "shallow_literal")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_shallow_literal(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_shallow_literal(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_DEEP_LITERAL, "routerama_static/hits", "deep_literal")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_deep_literal(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_deep_literal(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_FANOUT_FIRST, "routerama_static/hits", "fanout_first")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_fanout_first(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_fanout_first(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_FANOUT_MIDDLE, "routerama_static/hits", "fanout_middle")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_fanout_middle(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_fanout_middle(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_FANOUT_LAST, "routerama_static/hits", "fanout_last")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_fanout_last(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_fanout_last(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_BORROW_ONE, "routerama_static/hits", "borrow_one")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_borrow_one(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_borrow_one(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_BORROW_FOUR, "routerama_static/hits", "borrow_four")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_borrow_four(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_borrow_four(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_PARSE_NUMBER, "routerama_static/hits", "parse_number")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_parse_number(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_parse_number(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_OWN_PLAIN, "routerama_static/hits", "own_plain")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_own_plain(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_own_plain(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_HITS_OWN_PERCENT, "routerama_static/hits", "own_percent")]
#[bench::run(build_static_scenario())]
fn routerama_static_hits_own_percent(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_own_percent(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_MISSES_EARLY, "routerama_static/misses", "early")]
#[bench::run(build_static_scenario())]
fn routerama_static_misses_early(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_early_miss(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_MISSES_LATE, "routerama_static/misses", "late")]
#[bench::run(build_static_scenario())]
fn routerama_static_misses_late(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_late_miss(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_MISSES_PATHOLOGICAL_LONG, "routerama_static/misses", "pathological_long")]
#[bench::run(build_static_scenario())]
fn routerama_static_misses_pathological_long(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_pathological_long_miss(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_MISSES_WRONG_METHOD, "routerama_static/misses", "wrong_method")]
#[bench::run(build_static_scenario())]
fn routerama_static_misses_wrong_method(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_wrong_method(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_FEATURES_REST, "routerama_static/features", "rest")]
#[bench::run(build_static_scenario())]
fn routerama_static_features_rest(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_rest(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_FEATURES_AFFIX, "routerama_static/features", "affix")]
#[bench::run(build_static_scenario())]
fn routerama_static_features_affix(router: StaticScenarioRouter) -> StaticScenarioRouter {
    static_affix(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_FEATURES_NO_VERB_TABLE, "routerama_static/features", "no_verb_table")]
#[bench::run(build_no_verb_scenario())]
fn routerama_static_features_no_verb_table(router: NoVerbScenarioRouter) -> NoVerbScenarioRouter {
    static_no_verb(&router);
    router
}

#[metabench::benchmark(
    ROUTERAMA_STATIC_FEATURES_VERB_TABLE_NONVERB_HIT,
    "routerama_static/features",
    "verb_table_nonverb_hit"
)]
#[bench::run(build_with_verb_scenario())]
fn routerama_static_features_verb_table_nonverb_hit(router: WithVerbScenarioRouter) -> WithVerbScenarioRouter {
    static_with_verb_nonverb_hit(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_STATIC_FEATURES_VERB_HIT, "routerama_static/features", "verb_hit")]
#[bench::run(build_with_verb_scenario())]
fn routerama_static_features_verb_hit(router: WithVerbScenarioRouter) -> WithVerbScenarioRouter {
    static_with_verb_hit(&router);
    router
}

#[metabench::benchmark(
    ROUTERAMA_STATIC_TABLE_SHAPE_SHALLOW_TABLE_HIT,
    "routerama_static/table_shape",
    "shallow_table_hit"
)]
#[bench::run(build_shallow_table())]
fn routerama_static_table_shape_shallow_table_hit(router: ShallowTableRouter) -> ShallowTableRouter {
    static_shallow_table_hit(&router);
    router
}

#[metabench::benchmark(
    ROUTERAMA_STATIC_TABLE_SHAPE_DEEP_OUTLIER_TABLE_HIT,
    "routerama_static/table_shape",
    "deep_outlier_table_hit"
)]
#[bench::run(build_deep_outlier_table())]
fn routerama_static_table_shape_deep_outlier_table_hit(router: DeepOutlierTableRouter) -> DeepOutlierTableRouter {
    static_deep_outlier_table_hit(&router);
    router
}

#[metabench::benchmark(
    ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_FIRST,
    "routerama_static/table_shape",
    "affix_fanout_first"
)]
#[bench::run(build_affix_fanout())]
fn routerama_static_table_shape_affix_fanout_first(router: AffixFanoutRouter) -> AffixFanoutRouter {
    static_affix_fanout_first(&router);
    router
}

#[metabench::benchmark(
    ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_MIDDLE,
    "routerama_static/table_shape",
    "affix_fanout_middle"
)]
#[bench::run(build_affix_fanout())]
fn routerama_static_table_shape_affix_fanout_middle(router: AffixFanoutRouter) -> AffixFanoutRouter {
    static_affix_fanout_middle(&router);
    router
}

#[metabench::benchmark(
    ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_LAST,
    "routerama_static/table_shape",
    "affix_fanout_last"
)]
#[bench::run(build_affix_fanout())]
fn routerama_static_table_shape_affix_fanout_last(router: AffixFanoutRouter) -> AffixFanoutRouter {
    static_affix_fanout_last(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_TYPED_UNIT, "routerama_dynamic/typed", "unit")]
#[bench::run(build_dynamic_typed())]
fn routerama_dynamic_typed_unit(scenario: DynamicTypedScenarioResolver) -> DynamicTypedScenarioResolver {
    dynamic_typed_unit(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_TYPED_PARSE, "routerama_dynamic/typed", "parse")]
#[bench::run(build_dynamic_typed())]
fn routerama_dynamic_typed_parse(scenario: DynamicTypedScenarioResolver) -> DynamicTypedScenarioResolver {
    dynamic_typed_parse(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_TYPED_OWNED_PLAIN, "routerama_dynamic/typed", "owned_plain")]
#[bench::run(build_dynamic_typed())]
fn routerama_dynamic_typed_owned_plain(scenario: DynamicTypedScenarioResolver) -> DynamicTypedScenarioResolver {
    dynamic_typed_owned_plain(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_TYPED_OWNED_PERCENT, "routerama_dynamic/typed", "owned_percent")]
#[bench::run(build_dynamic_typed())]
fn routerama_dynamic_typed_owned_percent(scenario: DynamicTypedScenarioResolver) -> DynamicTypedScenarioResolver {
    dynamic_typed_owned_percent(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FANOUT_1, "routerama_dynamic/fanout", "1")]
#[bench::run(build_dynamic_fanout(1))]
fn routerama_dynamic_fanout_1(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_fanout_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FANOUT_2, "routerama_dynamic/fanout", "2")]
#[bench::run(build_dynamic_fanout(2))]
fn routerama_dynamic_fanout_2(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_fanout_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FANOUT_4, "routerama_dynamic/fanout", "4")]
#[bench::run(build_dynamic_fanout(4))]
fn routerama_dynamic_fanout_4(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_fanout_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FANOUT_8, "routerama_dynamic/fanout", "8")]
#[bench::run(build_dynamic_fanout(8))]
fn routerama_dynamic_fanout_8(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_fanout_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FANOUT_16, "routerama_dynamic/fanout", "16")]
#[bench::run(build_dynamic_fanout(16))]
fn routerama_dynamic_fanout_16(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_fanout_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FANOUT_32, "routerama_dynamic/fanout", "32")]
#[bench::run(build_dynamic_fanout(32))]
fn routerama_dynamic_fanout_32(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_fanout_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FANOUT_64, "routerama_dynamic/fanout", "64")]
#[bench::run(build_dynamic_fanout(64))]
fn routerama_dynamic_fanout_64(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_fanout_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_CAPTURE_COUNT_4, "routerama_dynamic/capture_count", "4")]
#[bench::run(build_capture_threshold(4))]
fn routerama_dynamic_capture_count_4(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_capture_threshold_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_CAPTURE_COUNT_5, "routerama_dynamic/capture_count", "5")]
#[bench::run(build_capture_threshold(5))]
fn routerama_dynamic_capture_count_5(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_capture_threshold_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_MISSES_EARLY, "routerama_dynamic/misses", "early")]
#[bench::run(build_dynamic_misses())]
fn routerama_dynamic_misses_early(scenario: RawResolver) -> RawResolver {
    dynamic_early_miss(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_MISSES_LATE, "routerama_dynamic/misses", "late")]
#[bench::run(build_dynamic_misses())]
fn routerama_dynamic_misses_late(scenario: RawResolver) -> RawResolver {
    dynamic_late_miss(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_MISSES_WRONG_METHOD, "routerama_dynamic/misses", "wrong_method")]
#[bench::run(build_dynamic_misses())]
fn routerama_dynamic_misses_wrong_method(scenario: RawResolver) -> RawResolver {
    dynamic_wrong_method(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FEATURES_REST, "routerama_dynamic/features", "rest")]
#[bench::run(build_dynamic_features())]
fn routerama_dynamic_features_rest(scenario: RawResolver) -> RawResolver {
    dynamic_rest(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FEATURES_AFFIX, "routerama_dynamic/features", "affix")]
#[bench::run(build_dynamic_features())]
fn routerama_dynamic_features_affix(scenario: RawResolver) -> RawResolver {
    dynamic_affix(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FEATURES_NO_VERB_TABLE, "routerama_dynamic/features", "no_verb_table")]
#[bench::run(build_dynamic_no_verb())]
fn routerama_dynamic_features_no_verb_table(scenario: RawResolver) -> RawResolver {
    dynamic_no_verb(&scenario);
    scenario
}

#[metabench::benchmark(
    ROUTERAMA_DYNAMIC_FEATURES_VERB_TABLE_NONVERB_HIT,
    "routerama_dynamic/features",
    "verb_table_nonverb_hit"
)]
#[bench::run(build_dynamic_with_verb())]
fn routerama_dynamic_features_verb_table_nonverb_hit(scenario: RawResolver) -> RawResolver {
    dynamic_with_verb_nonverb_hit(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_FEATURES_VERB_HIT, "routerama_dynamic/features", "verb_hit")]
#[bench::run(build_dynamic_with_verb())]
fn routerama_dynamic_features_verb_hit(scenario: RawResolver) -> RawResolver {
    dynamic_verb_hit(&scenario);
    scenario
}

#[metabench::benchmark(
    ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_SHALLOW_IN_16_TABLE,
    "routerama_dynamic/segment_depth",
    "shallow_in_16_table"
)]
#[bench::run(build_dynamic_depth(16))]
fn routerama_dynamic_segment_depth_shallow_in_16_table(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_depth_table_shallow_lookup(&scenario);
    scenario
}

#[metabench::benchmark(
    ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_SHALLOW_IN_17_TABLE,
    "routerama_dynamic/segment_depth",
    "shallow_in_17_table"
)]
#[bench::run(build_dynamic_depth(17))]
fn routerama_dynamic_segment_depth_shallow_in_17_table(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_depth_table_shallow_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_DEEP_16, "routerama_dynamic/segment_depth", "deep_16")]
#[bench::run(build_dynamic_depth(16))]
fn routerama_dynamic_segment_depth_deep_16(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_depth_table_deep_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_DEEP_17, "routerama_dynamic/segment_depth", "deep_17")]
#[bench::run(build_dynamic_depth(17))]
fn routerama_dynamic_segment_depth_deep_17(scenario: (RawResolver, String)) -> (RawResolver, String) {
    dynamic_depth_table_deep_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_DEEP_SCRATCH_SHALLOW_LOOKUP, "routerama_dynamic/deep_scratch", "shallow_lookup")]
#[bench::run(build_deep_dynamic())]
fn routerama_dynamic_deep_scratch_shallow_lookup(scenario: RawResolver) -> RawResolver {
    dynamic_deep_table_shallow_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_DYNAMIC_DEEP_SCRATCH_DEEP_LOOKUP, "routerama_dynamic/deep_scratch", "deep_lookup")]
#[bench::run(build_deep_dynamic())]
fn routerama_dynamic_deep_scratch_deep_lookup(scenario: RawResolver) -> RawResolver {
    dynamic_deep_table_deep_lookup(&scenario);
    scenario
}

#[metabench::benchmark(ROUTERAMA_MIXED_DISPATCH_STATIC_HIT, "routerama_mixed/dispatch", "static_hit")]
#[bench::run(build_mixed_scenario())]
fn routerama_mixed_dispatch_static_hit(router: MixedScenarioResolver) -> MixedScenarioResolver {
    mixed_static_hit(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_MIXED_DISPATCH_DYNAMIC_FALLBACK_HIT, "routerama_mixed/dispatch", "dynamic_fallback_hit")]
#[bench::run(build_mixed_scenario())]
fn routerama_mixed_dispatch_dynamic_fallback_hit(router: MixedScenarioResolver) -> MixedScenarioResolver {
    mixed_dynamic_hit(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_MIXED_DISPATCH_COMPLETE_MISS, "routerama_mixed/dispatch", "complete_miss")]
#[bench::run(build_mixed_scenario())]
fn routerama_mixed_dispatch_complete_miss(router: MixedScenarioResolver) -> MixedScenarioResolver {
    mixed_complete_miss(&router);
    router
}

#[metabench::benchmark(ROUTERAMA_MIXED_DISPATCH_STATIC_CAPTURE_ERROR, "routerama_mixed/dispatch", "static_capture_error")]
#[bench::run(build_mixed_scenario())]
fn routerama_mixed_dispatch_static_capture_error(router: MixedScenarioResolver) -> MixedScenarioResolver {
    mixed_static_capture_error(&router);
    router
}

#[cfg(feature = "query")]
#[metabench::benchmark(ROUTERAMA_QUERY_PARSE_COMMON_ROUTERAMA, "routerama_query/parse_common", "routerama")]
fn routerama_query_parse_common_routerama() {
    direct_parse_common();
}

#[cfg(feature = "query")]
#[metabench::benchmark(ROUTERAMA_QUERY_PARSE_COMMON_SERDE_URLENCODED, "routerama_query/parse_common", "serde_urlencoded")]
fn routerama_query_parse_common_serde_urlencoded() {
    serde_urlencoded_parse_common();
}

#[cfg(feature = "query")]
#[metabench::benchmark(ROUTERAMA_QUERY_PARSE_COMMON_SERDE_HTML_FORM, "routerama_query/parse_common", "serde_html_form")]
fn routerama_query_parse_common_serde_html_form() {
    serde_html_form_parse_common();
}

#[cfg(feature = "query")]
#[metabench::benchmark(ROUTERAMA_QUERY_PARSE_ESCAPED_ROUTERAMA, "routerama_query/parse_escaped", "routerama")]
fn routerama_query_parse_escaped_routerama() {
    direct_parse_escaped();
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PARSE_ESCAPED_SERDE_URLENCODED,
    "routerama_query/parse_escaped",
    "serde_urlencoded"
)]
fn routerama_query_parse_escaped_serde_urlencoded() {
    serde_urlencoded_parse_escaped();
}

#[cfg(feature = "query")]
#[metabench::benchmark(ROUTERAMA_QUERY_PARSE_ESCAPED_SERDE_HTML_FORM, "routerama_query/parse_escaped", "serde_html_form")]
fn routerama_query_parse_escaped_serde_html_form() {
    serde_html_form_parse_escaped();
}

#[cfg(feature = "query")]
#[metabench::benchmark(ROUTERAMA_QUERY_PARSE_REPEATED_ROUTERAMA, "routerama_query/parse_repeated", "routerama")]
fn routerama_query_parse_repeated_routerama() {
    direct_parse_repeated();
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PARSE_REPEATED_SERDE_HTML_FORM,
    "routerama_query/parse_repeated",
    "serde_html_form"
)]
fn routerama_query_parse_repeated_serde_html_form() {
    serde_html_form_parse_repeated();
}

#[cfg(feature = "query")]
#[metabench::benchmark(ROUTERAMA_QUERY_PARSE_LONG_ASCII_ROUTERAMA, "routerama_query/parse_long_ascii", "routerama")]
fn routerama_query_parse_long_ascii_routerama() {
    direct_parse_long();
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PARSE_LONG_ASCII_SERDE_URLENCODED,
    "routerama_query/parse_long_ascii",
    "serde_urlencoded"
)]
fn routerama_query_parse_long_ascii_serde_urlencoded() {
    serde_urlencoded_parse_long();
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PARSE_LONG_ASCII_SERDE_HTML_FORM,
    "routerama_query/parse_long_ascii",
    "serde_html_form"
)]
fn routerama_query_parse_long_ascii_serde_html_form() {
    serde_html_form_parse_long();
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PRODUCE_COMMON_ROUTERAMA_RESERVED,
    "routerama_query/produce_common",
    "routerama_reserved"
)]
fn routerama_query_produce_common_routerama_reserved() {
    let query = direct_common_value();
    let mut output = String::with_capacity(64);
    direct_produce_common(&query, &mut output);
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PRODUCE_COMMON_SERDE_HTML_FORM_RESERVED,
    "routerama_query/produce_common",
    "serde_html_form_reserved"
)]
fn routerama_query_produce_common_serde_html_form_reserved() {
    let query = serde_common_value();
    let mut output = String::with_capacity(64);
    serde_html_form_produce_common_reserved(&query, &mut output);
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PRODUCE_COMMON_ALLOCATING_ROUTERAMA,
    "routerama_query/produce_common_allocating",
    "routerama"
)]
fn routerama_query_produce_common_allocating_routerama() {
    direct_produce_common_allocating(&direct_common_value());
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PRODUCE_COMMON_ALLOCATING_SERDE_URLENCODED,
    "routerama_query/produce_common_allocating",
    "serde_urlencoded"
)]
fn routerama_query_produce_common_allocating_serde_urlencoded() {
    serde_urlencoded_produce_common(&serde_common_value());
}

#[cfg(feature = "query")]
#[metabench::benchmark(
    ROUTERAMA_QUERY_PRODUCE_COMMON_ALLOCATING_SERDE_HTML_FORM,
    "routerama_query/produce_common_allocating",
    "serde_html_form"
)]
fn routerama_query_produce_common_allocating_serde_html_form() {
    serde_html_form_produce_common(&serde_common_value());
}

fn callgrind_branch_cache_config() -> LibraryBenchmarkConfig {
    let mut config = LibraryBenchmarkConfig::default();
    config.tool(Callgrind::with_args(["--branch-sim=yes", "--cache-sim=yes"]));
    config
}

fn callgrind_branch_config() -> LibraryBenchmarkConfig {
    let mut config = LibraryBenchmarkConfig::default();
    config.tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    config
}

#[cfg(feature = "query")]
metabench::main!(
    criterion = criterion_benchmarks,
    groups = {
        COMPARE_BENCHMARKS {
            benchmarks = [
                COMPARE_ROUTERS_ROUTERAMA_STATIC,
                COMPARE_ROUTERS_ROUTERAMA_DYNAMIC,
                COMPARE_ROUTERS_MATCHIT,
                COMPARE_ROUTERS_PATH_TREE,
                COMPARE_ROUTERS_REGEX,
                COMPARE_ROUTERS_ROUTE_RECOGNIZER,
                COMPARE_ROUTER_MISSES_ROUTERAMA_STATIC,
                COMPARE_ROUTER_MISSES_ROUTERAMA_DYNAMIC,
                COMPARE_ROUTER_MISSES_MATCHIT,
                COMPARE_ROUTER_MISSES_PATH_TREE,
                COMPARE_ROUTER_MISSES_REGEX,
                COMPARE_ROUTER_MISSES_ROUTE_RECOGNIZER,
            ],
            gungraun_config = callgrind_branch_cache_config(),
        },
        STATIC_BENCHMARKS {
            benchmarks = [
                ROUTERAMA_STATIC_HITS_SHALLOW_LITERAL,
                ROUTERAMA_STATIC_HITS_DEEP_LITERAL,
                ROUTERAMA_STATIC_HITS_FANOUT_FIRST,
                ROUTERAMA_STATIC_HITS_FANOUT_MIDDLE,
                ROUTERAMA_STATIC_HITS_FANOUT_LAST,
                ROUTERAMA_STATIC_HITS_BORROW_ONE,
                ROUTERAMA_STATIC_HITS_BORROW_FOUR,
                ROUTERAMA_STATIC_HITS_PARSE_NUMBER,
                ROUTERAMA_STATIC_HITS_OWN_PLAIN,
                ROUTERAMA_STATIC_HITS_OWN_PERCENT,
                ROUTERAMA_STATIC_MISSES_EARLY,
                ROUTERAMA_STATIC_MISSES_LATE,
                ROUTERAMA_STATIC_MISSES_PATHOLOGICAL_LONG,
                ROUTERAMA_STATIC_MISSES_WRONG_METHOD,
                ROUTERAMA_STATIC_FEATURES_REST,
                ROUTERAMA_STATIC_FEATURES_AFFIX,
                ROUTERAMA_STATIC_FEATURES_NO_VERB_TABLE,
                ROUTERAMA_STATIC_FEATURES_VERB_TABLE_NONVERB_HIT,
                ROUTERAMA_STATIC_FEATURES_VERB_HIT,
                ROUTERAMA_STATIC_TABLE_SHAPE_SHALLOW_TABLE_HIT,
                ROUTERAMA_STATIC_TABLE_SHAPE_DEEP_OUTLIER_TABLE_HIT,
                ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_FIRST,
                ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_MIDDLE,
                ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_LAST,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        DYNAMIC_BENCHMARKS {
            benchmarks = [
                ROUTERAMA_DYNAMIC_TYPED_UNIT,
                ROUTERAMA_DYNAMIC_TYPED_PARSE,
                ROUTERAMA_DYNAMIC_TYPED_OWNED_PLAIN,
                ROUTERAMA_DYNAMIC_TYPED_OWNED_PERCENT,
                ROUTERAMA_DYNAMIC_FANOUT_1,
                ROUTERAMA_DYNAMIC_FANOUT_2,
                ROUTERAMA_DYNAMIC_FANOUT_4,
                ROUTERAMA_DYNAMIC_FANOUT_8,
                ROUTERAMA_DYNAMIC_FANOUT_16,
                ROUTERAMA_DYNAMIC_FANOUT_32,
                ROUTERAMA_DYNAMIC_FANOUT_64,
                ROUTERAMA_DYNAMIC_CAPTURE_COUNT_4,
                ROUTERAMA_DYNAMIC_CAPTURE_COUNT_5,
                ROUTERAMA_DYNAMIC_MISSES_EARLY,
                ROUTERAMA_DYNAMIC_MISSES_LATE,
                ROUTERAMA_DYNAMIC_MISSES_WRONG_METHOD,
                ROUTERAMA_DYNAMIC_FEATURES_REST,
                ROUTERAMA_DYNAMIC_FEATURES_AFFIX,
                ROUTERAMA_DYNAMIC_FEATURES_NO_VERB_TABLE,
                ROUTERAMA_DYNAMIC_FEATURES_VERB_TABLE_NONVERB_HIT,
                ROUTERAMA_DYNAMIC_FEATURES_VERB_HIT,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_SHALLOW_IN_16_TABLE,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_SHALLOW_IN_17_TABLE,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_DEEP_16,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_DEEP_17,
                ROUTERAMA_DYNAMIC_DEEP_SCRATCH_SHALLOW_LOOKUP,
                ROUTERAMA_DYNAMIC_DEEP_SCRATCH_DEEP_LOOKUP,
            ],
            gungraun_config = callgrind_branch_cache_config(),
        },
        MIXED_BENCHMARKS {
            benchmarks = [
                ROUTERAMA_MIXED_DISPATCH_STATIC_HIT,
                ROUTERAMA_MIXED_DISPATCH_DYNAMIC_FALLBACK_HIT,
                ROUTERAMA_MIXED_DISPATCH_COMPLETE_MISS,
                ROUTERAMA_MIXED_DISPATCH_STATIC_CAPTURE_ERROR,
            ],
            gungraun_config = callgrind_branch_cache_config(),
        },
        QUERY_BENCHMARKS {
            benchmarks = [
                ROUTERAMA_QUERY_PARSE_COMMON_ROUTERAMA,
                ROUTERAMA_QUERY_PARSE_COMMON_SERDE_URLENCODED,
                ROUTERAMA_QUERY_PARSE_COMMON_SERDE_HTML_FORM,
                ROUTERAMA_QUERY_PARSE_ESCAPED_ROUTERAMA,
                ROUTERAMA_QUERY_PARSE_ESCAPED_SERDE_URLENCODED,
                ROUTERAMA_QUERY_PARSE_ESCAPED_SERDE_HTML_FORM,
                ROUTERAMA_QUERY_PARSE_REPEATED_ROUTERAMA,
                ROUTERAMA_QUERY_PARSE_REPEATED_SERDE_HTML_FORM,
                ROUTERAMA_QUERY_PARSE_LONG_ASCII_ROUTERAMA,
                ROUTERAMA_QUERY_PARSE_LONG_ASCII_SERDE_URLENCODED,
                ROUTERAMA_QUERY_PARSE_LONG_ASCII_SERDE_HTML_FORM,
                ROUTERAMA_QUERY_PRODUCE_COMMON_ROUTERAMA_RESERVED,
                ROUTERAMA_QUERY_PRODUCE_COMMON_SERDE_HTML_FORM_RESERVED,
                ROUTERAMA_QUERY_PRODUCE_COMMON_ALLOCATING_ROUTERAMA,
                ROUTERAMA_QUERY_PRODUCE_COMMON_ALLOCATING_SERDE_URLENCODED,
                ROUTERAMA_QUERY_PRODUCE_COMMON_ALLOCATING_SERDE_HTML_FORM,
            ],
        },
    },
);

#[cfg(not(feature = "query"))]
metabench::main!(
    criterion = criterion_benchmarks,
    groups = {
        COMPARE_BENCHMARKS {
            benchmarks = [
                COMPARE_ROUTERS_ROUTERAMA_STATIC,
                COMPARE_ROUTERS_ROUTERAMA_DYNAMIC,
                COMPARE_ROUTERS_MATCHIT,
                COMPARE_ROUTERS_PATH_TREE,
                COMPARE_ROUTERS_REGEX,
                COMPARE_ROUTERS_ROUTE_RECOGNIZER,
                COMPARE_ROUTER_MISSES_ROUTERAMA_STATIC,
                COMPARE_ROUTER_MISSES_ROUTERAMA_DYNAMIC,
                COMPARE_ROUTER_MISSES_MATCHIT,
                COMPARE_ROUTER_MISSES_PATH_TREE,
                COMPARE_ROUTER_MISSES_REGEX,
                COMPARE_ROUTER_MISSES_ROUTE_RECOGNIZER,
            ],
            gungraun_config = callgrind_branch_cache_config(),
        },
        STATIC_BENCHMARKS {
            benchmarks = [
                ROUTERAMA_STATIC_HITS_SHALLOW_LITERAL,
                ROUTERAMA_STATIC_HITS_DEEP_LITERAL,
                ROUTERAMA_STATIC_HITS_FANOUT_FIRST,
                ROUTERAMA_STATIC_HITS_FANOUT_MIDDLE,
                ROUTERAMA_STATIC_HITS_FANOUT_LAST,
                ROUTERAMA_STATIC_HITS_BORROW_ONE,
                ROUTERAMA_STATIC_HITS_BORROW_FOUR,
                ROUTERAMA_STATIC_HITS_PARSE_NUMBER,
                ROUTERAMA_STATIC_HITS_OWN_PLAIN,
                ROUTERAMA_STATIC_HITS_OWN_PERCENT,
                ROUTERAMA_STATIC_MISSES_EARLY,
                ROUTERAMA_STATIC_MISSES_LATE,
                ROUTERAMA_STATIC_MISSES_PATHOLOGICAL_LONG,
                ROUTERAMA_STATIC_MISSES_WRONG_METHOD,
                ROUTERAMA_STATIC_FEATURES_REST,
                ROUTERAMA_STATIC_FEATURES_AFFIX,
                ROUTERAMA_STATIC_FEATURES_NO_VERB_TABLE,
                ROUTERAMA_STATIC_FEATURES_VERB_TABLE_NONVERB_HIT,
                ROUTERAMA_STATIC_FEATURES_VERB_HIT,
                ROUTERAMA_STATIC_TABLE_SHAPE_SHALLOW_TABLE_HIT,
                ROUTERAMA_STATIC_TABLE_SHAPE_DEEP_OUTLIER_TABLE_HIT,
                ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_FIRST,
                ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_MIDDLE,
                ROUTERAMA_STATIC_TABLE_SHAPE_AFFIX_FANOUT_LAST,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        DYNAMIC_BENCHMARKS {
            benchmarks = [
                ROUTERAMA_DYNAMIC_TYPED_UNIT,
                ROUTERAMA_DYNAMIC_TYPED_PARSE,
                ROUTERAMA_DYNAMIC_TYPED_OWNED_PLAIN,
                ROUTERAMA_DYNAMIC_TYPED_OWNED_PERCENT,
                ROUTERAMA_DYNAMIC_FANOUT_1,
                ROUTERAMA_DYNAMIC_FANOUT_2,
                ROUTERAMA_DYNAMIC_FANOUT_4,
                ROUTERAMA_DYNAMIC_FANOUT_8,
                ROUTERAMA_DYNAMIC_FANOUT_16,
                ROUTERAMA_DYNAMIC_FANOUT_32,
                ROUTERAMA_DYNAMIC_FANOUT_64,
                ROUTERAMA_DYNAMIC_CAPTURE_COUNT_4,
                ROUTERAMA_DYNAMIC_CAPTURE_COUNT_5,
                ROUTERAMA_DYNAMIC_MISSES_EARLY,
                ROUTERAMA_DYNAMIC_MISSES_LATE,
                ROUTERAMA_DYNAMIC_MISSES_WRONG_METHOD,
                ROUTERAMA_DYNAMIC_FEATURES_REST,
                ROUTERAMA_DYNAMIC_FEATURES_AFFIX,
                ROUTERAMA_DYNAMIC_FEATURES_NO_VERB_TABLE,
                ROUTERAMA_DYNAMIC_FEATURES_VERB_TABLE_NONVERB_HIT,
                ROUTERAMA_DYNAMIC_FEATURES_VERB_HIT,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_SHALLOW_IN_16_TABLE,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_SHALLOW_IN_17_TABLE,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_DEEP_16,
                ROUTERAMA_DYNAMIC_SEGMENT_DEPTH_DEEP_17,
                ROUTERAMA_DYNAMIC_DEEP_SCRATCH_SHALLOW_LOOKUP,
                ROUTERAMA_DYNAMIC_DEEP_SCRATCH_DEEP_LOOKUP,
            ],
            gungraun_config = callgrind_branch_cache_config(),
        },
        MIXED_BENCHMARKS {
            benchmarks = [
                ROUTERAMA_MIXED_DISPATCH_STATIC_HIT,
                ROUTERAMA_MIXED_DISPATCH_DYNAMIC_FALLBACK_HIT,
                ROUTERAMA_MIXED_DISPATCH_COMPLETE_MISS,
                ROUTERAMA_MIXED_DISPATCH_STATIC_CAPTURE_ERROR,
            ],
            gungraun_config = callgrind_branch_cache_config(),
        },
    },
);
