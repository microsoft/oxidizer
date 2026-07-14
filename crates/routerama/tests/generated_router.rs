// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Correctness / drift guard for the committed benchmark router fixture.
//!
//! `benches/common/generated_router.rs` is generated from the route table in
//! `benches/common/routes_data.rs` (via `scripts/perf_report.rs
//! --regenerate-router`) and committed so the benchmarks need not enable the
//! `build` feature. These tests fail if the committed router drifts out of
//! sync with the route table, keeping the benchmark honest.

#![allow(dead_code, reason = "the included route table also defines LOOKUPS, used only by the benches")]

include!("../benches/common/routes_data.rs");
include!("../benches/common/generated_router.rs");

use routerama::{Resolver as _, RouteMatch as _};

/// Fills a `{var}` template with placeholder segment values, yielding a concrete
/// request path that should resolve back to that route.
fn fill(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    for segment in template.split('/') {
        if segment.is_empty() {
            continue;
        }
        out.push('/');
        if segment.starts_with('{') && segment.ends_with('}') {
            out.push('x');
        } else {
            out.push_str(segment);
        }
    }
    out
}

#[test]
fn generated_router_matches_the_route_table() {
    // Every route in the table resolves — via the committed generated router —
    // back to its own name. If `routes_data.rs` changed without regenerating the
    // router, this fails.
    for (name, template) in ROUTES {
        let path = fill(template);
        let matched = RouteResolver
            .resolve("GET", &path)
            .unwrap_or_else(|| panic!("no route matched `{path}` (from template `{template}`)"));
        assert_eq!(matched.name(), *name, "template `{template}` resolved to the wrong route");
    }
}

#[test]
fn every_benchmark_lookup_hits_a_route() {
    // Each benchmark lookup path is a genuine hit, so the "compare routers"
    // benchmark measures matching (not missing) across every router.
    for path in LOOKUPS {
        assert!(
            RouteResolver.resolve("GET", path).is_some(),
            "benchmark lookup `{path}` does not match any route"
        );
    }
}
