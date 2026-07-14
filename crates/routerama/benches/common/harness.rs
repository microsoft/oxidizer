// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared "compare routers" harness, `include!`d by both `criterion_routers.rs`
// and `gungraun_routers.rs`. It builds each router being compared from the same
// route table and exercises the same set of request-path lookups, so the two
// benchmark suites measure an identical workload (wall-clock vs. instruction
// counts) across the field.
//
// Fairness contract — held constant for every router:
//   * the same route table (`ROUTES`) and the same lookup paths (`LOOKUPS`),
//     every one of which is a hit;
//   * router construction happens in a setup step excluded from the measured
//     region (`routerama` needs none — its router is generated at build time,
//     see `common/generated_router.rs`);
//   * each measured iteration drives every router to the *same end state* that
//     `routerama`'s generated `resolve` reaches in one step: the route's HTTP
//     method (verb) has been validated against the request and every captured
//     path variable has been extracted into a usable `&str`. `routerama` gives
//     you this directly (a typed enum variant, method already matched); the
//     other routers only select a route, so the harness explicitly performs the
//     method check and pulls out each parameter afterwards (see `consume`), so
//     the comparison measures equivalent work rather than a bare route lookup;
//   * every extracted value is fed through `black_box`, and each hit is guarded
//     by a `debug_assert!` (compiled out in release, so it adds no measured
//     cost) taken through an `if let` rather than an `unwrap`/`expect`, so no
//     router is under-measured by dead-code elimination or over-measured by a
//     panic branch.
//
// One intrinsic difference remains, inherent to the library and noted for
// transparency rather than "corrected": `regex` routes with a `RegexSet`
// (membership) to select the winner and then re-scans it with the winning
// `Regex` to capture — two passes over the path — because a `RegexSet` cannot
// capture. It therefore does structurally more work than the trie routers; read
// it as an upper bound for a regex-based router that still reaches the same
// extracted end state.
//
// Per-router route-syntax differences are handled by `to_pattern`.

use std::hint::black_box;

include!("routes_data.rs");
include!("generated_router.rs");

/// The HTTP method every benchmark route is registered under and every lookup
/// requests. `routerama` matches it as part of `resolve`; the other routers are
/// path-only, so the harness stores it alongside each route value and validates
/// it after matching (see `consume`) to reach the same end state.
const REQUEST_METHOD: &str = "GET";

/// The value stored per route in the non-`routerama` routers: the route name and
/// its HTTP method, so a match can validate the verb and report the route.
type RouteValue = (&'static str, &'static str);

/// Drives a matched non-`routerama` route to `routerama`'s end state: validate
/// the verb against the request method and extract every captured variable.
///
/// `name`/`method` come from the matched route's stored [`RouteValue`]; `params`
/// yields the router's captured `(name, value)` pairs. Everything is forced
/// through `black_box` so the extraction cannot be elided.
#[inline]
fn consume<'a>(name: &str, method: &str, params: impl Iterator<Item = (&'a str, &'a str)>) {
    debug_assert!(method == REQUEST_METHOD, "unexpected route method {method}");
    black_box(name);
    black_box(method == REQUEST_METHOD);
    for (key, value) in params {
        black_box(key);
        black_box(value);
    }
}

/// The route-parameter syntax a given router expects.
#[derive(Clone, Copy)]
enum Style {
    /// `{p0}` bracketed parameters (matchit, actix-router).
    Brackets,
    /// `:p0` colon parameters (path-tree, route-recognizer, routefinder).
    Colon,
    /// A full anchored regular expression (regex `RegexSet`).
    Regex,
}

/// Rewrites a `{var}` route template into the `style` a given router expects,
/// numbering parameters positionally so no route carries a duplicate name and no
/// two routes disagree on a shared position (which some routers reject).
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

// --- routerama_static: the build-time generated router (no runtime construction). ---

fn routerama_static_lookups() {
    use ::routerama::Resolver as _;
    for path in LOOKUPS {
        let matched = RouteResolver.resolve("GET", black_box(path));
        debug_assert!(matched.is_some(), "routerama misses {path}");
        black_box(matched);
    }
}

// --- routerama dynamic: a runtime router built from the same route table. ---
// Built in the setup step (excluded from the measured region), it walks the same
// shared trie the static path lowers, so it resolves the identical route set.

#[cfg(feature = "dynamic")]
fn build_routerama_dynamic() -> ::routerama::DynResolver {
    use ::http_path_template::{Grammar, PathTemplate};
    use ::routerama::{DynResolver, HttpMethod, Route};
    DynResolver::new(
        ROUTES
            .iter()
            .map(|(name, template)| {
                Route::new(
                    *name,
                    HttpMethod::Get,
                    PathTemplate::parse(template, Grammar::default()).expect("valid template"),
                )
            }),
    )
}

#[cfg(feature = "dynamic")]
fn routerama_dynamic_lookups(router: &::routerama::DynResolver) {
    use ::routerama::Resolver as _;
    for path in LOOKUPS {
        let matched = router.resolve("GET", black_box(path));
        debug_assert!(matched.is_some(), "routerama dynamic misses {path}");
        black_box(matched);
    }
}

// --- matchit ---

fn build_matchit() -> ::matchit::Router<RouteValue> {
    let mut router = ::matchit::Router::new();
    for (name, template) in ROUTES {
        router
            .insert(to_pattern(template, Style::Brackets), (*name, REQUEST_METHOD))
            .expect("matchit insert");
    }
    router
}

fn matchit_lookups(router: &::matchit::Router<RouteValue>) {
    for path in LOOKUPS {
        let matched = router.at(black_box(path));
        debug_assert!(matched.is_ok(), "matchit misses {path}");
        if let Ok(found) = matched {
            let (name, method) = *found.value;
            consume(name, method, found.params.iter());
        }
    }
}

// --- path-tree ---

fn build_path_tree() -> ::path_tree::PathTree<RouteValue> {
    let mut tree = ::path_tree::PathTree::new();
    for (name, template) in ROUTES {
        let _ = tree.insert(&to_pattern(template, Style::Colon), (*name, REQUEST_METHOD));
    }
    tree
}

fn path_tree_lookups(tree: &::path_tree::PathTree<RouteValue>) {
    for path in LOOKUPS {
        let matched = tree.find(black_box(path));
        debug_assert!(matched.is_some(), "path-tree misses {path}");
        if let Some((value, matched_path)) = matched {
            let (name, method) = *value;
            consume(name, method, matched_path.params_iter());
        }
    }
}

// --- actix-router ---

fn build_actix() -> ::actix_router::Router<RouteValue> {
    let mut builder = ::actix_router::Router::<RouteValue>::build();
    for (name, template) in ROUTES {
        builder.path(to_pattern(template, Style::Brackets).as_str(), (*name, REQUEST_METHOD));
    }
    builder.finish()
}

fn actix_lookups(router: &::actix_router::Router<RouteValue>) {
    for path in LOOKUPS {
        // `recognize` needs an owned `Path`, into which it extracts the matched
        // parameters; constructing it is part of actix's per-request cost.
        let mut resource = ::actix_router::Path::new(*path);
        let matched = router.recognize(black_box(&mut resource));
        debug_assert!(matched.is_some(), "actix misses {path}");
        if let Some((value, _id)) = matched {
            let (name, method) = *value;
            consume(name, method, resource.iter());
        }
    }
}

// --- regex: `RegexSet` selects the winner, then the winning `Regex` captures. ---
// A `RegexSet` reports membership only, so a regex-based router that must reach
// the same extracted end state pays for a second pass: the winning pattern's
// `Regex::captures`. This does structurally more work than the trie routers.

/// A regex "router": a `RegexSet` for winner selection plus the per-route
/// `Regex`, name, and method used to validate the verb and capture variables.
struct RegexRouter {
    set: ::regex::RegexSet,
    regexes: Vec<::regex::Regex>,
    routes: Vec<RouteValue>,
}

fn build_regex() -> RegexRouter {
    let patterns: Vec<String> = ROUTES.iter().map(|(_, template)| to_pattern(template, Style::Regex)).collect();
    let set = ::regex::RegexSet::new(&patterns).expect("regex set");
    let regexes = patterns.iter().map(|pattern| ::regex::Regex::new(pattern).expect("regex")).collect();
    let routes = ROUTES.iter().map(|(name, _)| (*name, REQUEST_METHOD)).collect();
    RegexRouter { set, regexes, routes }
}

fn regex_lookups(router: &RegexRouter) {
    for path in LOOKUPS {
        let matches = router.set.matches(black_box(path));
        debug_assert!(matches.matched_any(), "regex misses {path}");
        if let Some(index) = matches.iter().next() {
            let (name, method) = router.routes[index];
            debug_assert!(method == REQUEST_METHOD, "unexpected route method {method}");
            black_box(name);
            black_box(method == REQUEST_METHOD);
            if let Some(captures) = router.regexes[index].captures(path) {
                // Capture group 0 is the whole match; groups 1.. are the variables.
                for group in captures.iter().skip(1).flatten() {
                    black_box(group.as_str());
                }
            }
        }
    }
}

// --- route-recognizer ---

fn build_route_recognizer() -> ::route_recognizer::Router<RouteValue> {
    let mut router = ::route_recognizer::Router::new();
    for (name, template) in ROUTES {
        router.add(&to_pattern(template, Style::Colon), (*name, REQUEST_METHOD));
    }
    router
}

fn route_recognizer_lookups(router: &::route_recognizer::Router<RouteValue>) {
    for path in LOOKUPS {
        let matched = router.recognize(black_box(path));
        debug_assert!(matched.is_ok(), "route-recognizer misses {path}");
        if let Ok(found) = matched {
            let (name, method) = **found.handler();
            consume(name, method, found.params().iter());
        }
    }
}

// --- routefinder ---

fn build_routefinder() -> ::routefinder::Router<RouteValue> {
    let mut router = ::routefinder::Router::new();
    for (name, template) in ROUTES {
        router.add(to_pattern(template, Style::Colon), (*name, REQUEST_METHOD)).expect("routefinder insert");
    }
    router
}

fn routefinder_lookups(router: &::routefinder::Router<RouteValue>) {
    for path in LOOKUPS {
        let matched = router.best_match(black_box(path));
        debug_assert!(matched.is_some(), "routefinder misses {path}");
        if let Some(found) = matched {
            let (name, method) = *found.handler();
            let captures = found.captures();
            consume(name, method, captures.iter());
        }
    }
}
