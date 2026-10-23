// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Drift guard for the committed typed benchmark router.
//!
//! `benches/common/bench_router.rs` holds two generated resolvers
//! from the route table in `benches/common/routes_data.rs` (via
//! `scripts/perf_report.rs --regenerate-router`). These tests fail if it drifts
//! out of sync: every route must resolve to a real variant, every benchmark
//! lookup must hit, and every dynamic route must be registered.

#![allow(dead_code, reason = "the included route table also defines items used only by the benches")]
#![allow(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

include!("../benches/common/routes_data.rs");
include!("../benches/common/bench_router.rs");

/// Fills each `{var}` with `1`, a value valid for every capture type in the
/// table (`&str`, `u32`, and `String`), yielding a concrete path for that route.
fn fill(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    for segment in template.split('/') {
        if segment.is_empty() {
            continue;
        }
        out.push('/');
        if segment.starts_with('{') && segment.ends_with('}') {
            out.push('1');
        } else {
            out.push_str(segment);
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

#[test]
fn static_resolver_resolves_every_route() {
    let resolver = BenchRoute::resolver();
    for (name, template, _tys) in ROUTES {
        let path = fill(template);
        assert!(
            resolver.resolve("GET", &path).is_ok(),
            "static benchmark resolver does not resolve route `{name}` (`{template}`)"
        );
    }
}

#[test]
fn dynamic_resolver_registers_and_resolves_every_route() {
    let resolver = build_bench_dyn();
    for (name, template, _tys) in ROUTES {
        let path = fill(template);
        assert!(
            resolver.resolve("GET", &path).is_ok(),
            "dynamic benchmark resolver does not resolve route `{name}` (`{template}`)"
        );
    }
}

#[test]
fn every_benchmark_lookup_hits_both_resolvers() {
    let static_resolver = BenchRoute::resolver();
    let dynamic_resolver = build_bench_dyn();
    for path in LOOKUPS {
        assert!(
            static_resolver.resolve("GET", path).is_ok(),
            "static benchmark resolver misses lookup `{path}`"
        );
        assert!(
            dynamic_resolver.resolve("GET", path).is_ok(),
            "dynamic benchmark resolver misses lookup `{path}`"
        );
    }
}
