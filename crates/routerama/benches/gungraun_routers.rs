// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind instruction-count "compare routers" benchmark.
//!
//! Mirrors `criterion_routers.rs` 1:1 — each Gungraun function `<variant>`
//! measures the same hit or miss request-path sweep as the corresponding
//! Criterion benchmark. Every router is built and primed in a setup step, and
//! returned from the measured function so neither construction nor drop is
//! counted. Requires Valgrind (Linux-only).

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(
    unreachable_pub,
    dead_code,
    reason = "the shared harness is `include!`d and its items are used selectively per bench"
)]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun benchmark inputs are passed and returned by value by the framework"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::{library_benchmark, library_benchmark_group};

    include!("common/harness.rs");

    // routerama_static: the compile-time `#[resolver]` typed router (zero-sized).
    #[library_benchmark]
    #[bench::run(build_hot_routerama_static())]
    fn routerama_static(router: BenchRouteRouter) -> BenchRouteRouter {
        routerama_static_lookups(&router);
        router
    }

    // routerama_dynamic: a run-time `#[resolver]` typed router built from the same table.
    #[library_benchmark]
    #[bench::run(build_hot_routerama_dynamic())]
    fn routerama_dynamic(router: BenchDynRouteRouter) -> BenchDynRouteRouter {
        routerama_dynamic_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_matchit())]
    fn matchit(router: ::matchit::Router<RouteValue>) -> ::matchit::Router<RouteValue> {
        matchit_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_path_tree())]
    fn path_tree(tree: ::path_tree::PathTree<RouteValue>) -> ::path_tree::PathTree<RouteValue> {
        path_tree_lookups(&tree);
        tree
    }

    #[library_benchmark]
    #[bench::run(build_hot_regex())]
    fn regex(router: RegexRouter) -> RegexRouter {
        regex_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_route_recognizer())]
    fn route_recognizer(router: ::route_recognizer::Router<RouteValue>) -> ::route_recognizer::Router<RouteValue> {
        route_recognizer_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_routerama_static())]
    fn misses_routerama_static(router: BenchRouteRouter) -> BenchRouteRouter {
        routerama_static_misses(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_routerama_dynamic())]
    fn misses_routerama_dynamic(router: BenchDynRouteRouter) -> BenchDynRouteRouter {
        routerama_dynamic_misses(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_matchit())]
    fn misses_matchit(router: ::matchit::Router<RouteValue>) -> ::matchit::Router<RouteValue> {
        matchit_misses(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_path_tree())]
    fn misses_path_tree(tree: ::path_tree::PathTree<RouteValue>) -> ::path_tree::PathTree<RouteValue> {
        path_tree_misses(&tree);
        tree
    }

    #[library_benchmark]
    #[bench::run(build_hot_regex())]
    fn misses_regex(router: RegexRouter) -> RegexRouter {
        regex_misses(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_hot_route_recognizer())]
    fn misses_route_recognizer(router: ::route_recognizer::Router<RouteValue>) -> ::route_recognizer::Router<RouteValue> {
        route_recognizer_misses(&router);
        router
    }

    library_benchmark_group!(
        name = compare_routers;
        benchmarks =
            routerama_static, routerama_dynamic, matchit, path_tree, regex, route_recognizer
    );
    library_benchmark_group!(
        name = compare_router_misses;
        benchmarks =
            misses_routerama_static,
            misses_routerama_dynamic,
            misses_matchit,
            misses_path_tree,
            misses_regex,
            misses_route_recognizer
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{compare_router_misses, compare_routers};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes", "--cache-sim=yes"]));
    library_benchmark_groups = compare_routers, compare_router_misses
);
