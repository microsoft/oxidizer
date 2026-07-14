// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind instruction-count "compare routers" benchmark.
//!
//! Mirrors `criterion_routers.rs` 1:1 — each gungraun function `<variant>`
//! measures the same request-path lookup sweep as the criterion
//! `compare_routers/<variant>` benchmark, so instruction counts line up with the
//! wall-clock timings. Runtime routers are built in a setup step (and returned
//! from the measured function so their drop is not counted); `routerama`'s
//! generated `resolve` needs no construction. Requires Valgrind (Linux-only).

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

    // routerama_static: the build-time generated router (no runtime construction).
    #[library_benchmark]
    fn routerama_static() {
        routerama_static_lookups();
    }

    // routerama_dynamic: a runtime router built from the same route table.
    #[cfg(feature = "dynamic")]
    #[library_benchmark]
    #[bench::run(build_routerama_dynamic())]
    fn routerama_dynamic(router: ::routerama::DynResolver) -> ::routerama::DynResolver {
        routerama_dynamic_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_matchit())]
    fn matchit(router: ::matchit::Router<RouteValue>) -> ::matchit::Router<RouteValue> {
        matchit_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_path_tree())]
    fn path_tree(tree: ::path_tree::PathTree<RouteValue>) -> ::path_tree::PathTree<RouteValue> {
        path_tree_lookups(&tree);
        tree
    }

    #[library_benchmark]
    #[bench::run(build_actix())]
    fn actix_router(router: ::actix_router::Router<RouteValue>) -> ::actix_router::Router<RouteValue> {
        actix_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_regex())]
    fn regex(router: RegexRouter) -> RegexRouter {
        regex_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_route_recognizer())]
    fn route_recognizer(router: ::route_recognizer::Router<RouteValue>) -> ::route_recognizer::Router<RouteValue> {
        route_recognizer_lookups(&router);
        router
    }

    #[library_benchmark]
    #[bench::run(build_routefinder())]
    fn routefinder(router: ::routefinder::Router<RouteValue>) -> ::routefinder::Router<RouteValue> {
        routefinder_lookups(&router);
        router
    }

    #[cfg(feature = "dynamic")]
    library_benchmark_group!(
        name = compare_routers;
        benchmarks =
            routerama_static, routerama_dynamic, matchit, path_tree, actix_router, regex, route_recognizer, routefinder
    );

    #[cfg(not(feature = "dynamic"))]
    library_benchmark_group!(
        name = compare_routers;
        benchmarks = routerama_static, matchit, path_tree, actix_router, regex, route_recognizer, routefinder
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::compare_routers;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes", "--cache-sim=yes"]));
    library_benchmark_groups = compare_routers
);
