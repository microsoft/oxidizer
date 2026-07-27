// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for the generated static router in the `rest_over_grpc_tests`
//! crate.
//!
//! Measures the instruction count of the build-time generated `resolve` (lowered
//! from the GitHub-like `bench_routes.rs` table) across shallow-hit, deep-hit,
//! catch-all, and miss shapes, plus a `matchit` sibling for the same paths so the
//! two routing implementations' instruction counts are directly comparable.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(
    unreachable_pub,
    reason = "ROUTES is re-included from bench_routes.rs inside the private `linux` module"
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
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::{library_benchmark, library_benchmark_group};
    use rest_over_grpc_tests::bench_router::Route;

    // The shared route table (`ROUTES`).
    include!("../bench_routes.rs");

    type MethodAwareRouter = matchit::Router<Vec<(&'static str, &'static str)>>;

    // Builds a method-aware `matchit` router from `ROUTES` (path-keyed trie, each
    // node carrying its `(method, rpc)` pairs, as `axum` layers method routing on
    // `matchit`). Runs in setup, so its allocation is not measured.
    fn build_matchit() -> MethodAwareRouter {
        let mut by_path: Vec<(String, Vec<(&'static str, &'static str)>)> = Vec::new();
        for (rpc, method, pattern) in ROUTES {
            let path = to_matchit_path(pattern);
            if let Some((_, methods)) = by_path.iter_mut().find(|(p, _)| *p == path) {
                methods.push((*method, *rpc));
            } else {
                by_path.push((path, vec![(*method, *rpc)]));
            }
        }
        let mut router = matchit::Router::new();
        for (path, methods) in by_path {
            router
                .insert(path, methods)
                .expect("benchmark paths insert into matchit without conflict");
        }
        router
    }

    // Converts a `google.api.http` template into `matchit` 0.8 syntax: `{var}` stays
    // `{var}` (dots sanitized to `_`), a trailing `{var=**}` becomes a `{*var}`
    // catch-all.
    fn to_matchit_path(pattern: &str) -> String {
        let mut out = String::with_capacity(pattern.len());
        for segment in pattern.split('/') {
            if segment.is_empty() {
                continue;
            }
            out.push('/');
            if let Some(inner) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                out.push('{');
                if let Some((name, "**")) = inner.split_once('=') {
                    out.push('*');
                    out.push_str(&name.replace('.', "_"));
                } else {
                    out.push_str(&inner.replace('.', "_"));
                }
                out.push('}');
            } else {
                out.push_str(segment);
            }
        }
        out
    }

    fn matchit_lookup(router: &MethodAwareRouter, method: &str, path: &str) -> Option<&'static str> {
        router.at(path).ok().and_then(|matched| {
            matched
                .value
                .iter()
                .find(|(candidate, _)| *candidate == method)
                .map(|(_, rpc)| *rpc)
        })
    }

    // Generated router: a shallow two-segment hit.
    #[library_benchmark]
    fn generated_shallow() {
        black_box(Route::resolve(black_box("GET"), black_box("/v1/users/octocat")));
    }

    // Generated router: a deep six-segment hit.
    #[library_benchmark]
    fn generated_deep() {
        black_box(Route::resolve(
            black_box("GET"),
            black_box("/v1/repos/rust-lang/cargo/issues/1347/comments/7"),
        ));
    }

    // Generated router: a `**` catch-all hit.
    #[library_benchmark]
    fn generated_catch_all() {
        black_box(Route::resolve(
            black_box("GET"),
            black_box("/v1/repos/rust-lang/cargo/contents/src/lib/mod.rs"),
        ));
    }

    // Generated router: a miss (no route matches).
    #[library_benchmark]
    fn generated_miss() {
        black_box(Route::resolve(black_box("GET"), black_box("/v1/unknown")));
    }

    // `matchit` sibling: shallow hit. Returns the router so its trie drop is not
    // counted in the measured region.
    #[library_benchmark]
    #[bench::shallow(build_matchit())]
    fn matchit_shallow(router: MethodAwareRouter) -> MethodAwareRouter {
        black_box(matchit_lookup(black_box(&router), black_box("GET"), black_box("/v1/users/octocat")));
        router
    }

    // `matchit` sibling: deep hit. Returns the router so its trie drop is not
    // counted in the measured region.
    #[library_benchmark]
    #[bench::deep(build_matchit())]
    fn matchit_deep(router: MethodAwareRouter) -> MethodAwareRouter {
        black_box(matchit_lookup(
            black_box(&router),
            black_box("GET"),
            black_box("/v1/repos/rust-lang/cargo/issues/1347/comments/7"),
        ));
        router
    }

    // `matchit` sibling: a `**` catch-all hit. Returns the router so its trie
    // drop is not counted in the measured region.
    #[library_benchmark]
    #[bench::catch_all(build_matchit())]
    fn matchit_catch_all(router: MethodAwareRouter) -> MethodAwareRouter {
        black_box(matchit_lookup(
            black_box(&router),
            black_box("GET"),
            black_box("/v1/repos/rust-lang/cargo/contents/src/lib/mod.rs"),
        ));
        router
    }

    // `matchit` sibling: a miss (no route matches). Returns the router so its
    // trie drop is not counted in the measured region.
    #[library_benchmark]
    #[bench::miss(build_matchit())]
    fn matchit_miss(router: MethodAwareRouter) -> MethodAwareRouter {
        black_box(matchit_lookup(black_box(&router), black_box("GET"), black_box("/v1/unknown")));
        router
    }

    library_benchmark_group!(
        name = router;
        benchmarks =
            generated_shallow, generated_deep, generated_catch_all, generated_miss,
            matchit_shallow, matchit_deep, matchit_catch_all, matchit_miss
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::router;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = router
);
