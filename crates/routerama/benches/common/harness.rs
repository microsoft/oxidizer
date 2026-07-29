// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared Criterion and Callgrind comparison harness. Each router uses the same
// route table and warmed lookup workload, including method validation and typed
// capture coercion. Dynamic routerama owns captures; static routerama may borrow
// them. The regex case performs separate selection and capture passes.

use std::hint::black_box;

include!("routes_data.rs");
include!("bench_router.rs");

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
