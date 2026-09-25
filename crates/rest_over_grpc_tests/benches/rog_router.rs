// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generated route lookup and a method-aware matchit comparison over the same route table.

#![allow(missing_docs, reason = "benchmark code has no public API")]
#![allow(clippy::unwrap_used, reason = "invalid benchmark route fixtures must fail the run")]
#![allow(unreachable_pub, reason = "the build-script route table is included in this benchmark")]
#![allow(clippy::needless_pass_by_value, reason = "Gungraun owns prepared routers")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "metabench emits Gungraun entry points"
)]

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion};
use rest_over_grpc_tests::bench_router::Route;

include!("../bench_routes.rs");

const GENERATED: &str = "rog_router/generated";
const MATCHIT: &str = "rog_router/matchit";

type MethodAwareRouter = matchit::Router<Vec<(&'static str, &'static str)>>;

fn to_matchit_path(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    for segment in pattern.split('/').filter(|segment| !segment.is_empty()) {
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

fn build_matchit() -> MethodAwareRouter {
    let mut by_path: Vec<(String, Vec<(&'static str, &'static str)>)> = Vec::new();
    for (rpc, method, pattern) in ROUTES {
        let path = to_matchit_path(pattern);
        if let Some((_, methods)) = by_path.iter_mut().find(|(candidate, _)| *candidate == path) {
            methods.push((*method, *rpc));
        } else {
            by_path.push((path, vec![(*method, *rpc)]));
        }
    }
    let mut router = matchit::Router::new();
    for (path, methods) in by_path {
        router.insert(path, methods).unwrap();
    }
    router
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

#[metabench::benchmark(G_SHALLOW, GENERATED, "shallow")]
fn generated_shallow() -> bool {
    Route::resolve(black_box("GET"), black_box("/v1/users/octocat")).is_some()
}

#[metabench::benchmark(G_DEEP, GENERATED, "deep")]
fn generated_deep() -> bool {
    Route::resolve(black_box("GET"), black_box("/v1/repos/rust-lang/cargo/issues/1347/comments/7")).is_some()
}

#[metabench::benchmark(G_CATCH_ALL, GENERATED, "catch_all")]
fn generated_catch_all() -> bool {
    Route::resolve(black_box("GET"), black_box("/v1/repos/rust-lang/cargo/contents/src/lib/mod.rs")).is_some()
}

#[metabench::benchmark(G_MISS, GENERATED, "miss")]
fn generated_miss() -> bool {
    Route::resolve(black_box("GET"), black_box("/v1/unknown")).is_some()
}

#[metabench::benchmark(M_SHALLOW, MATCHIT, "shallow")]
#[bench::case(build_matchit())]
fn matchit_shallow(router: MethodAwareRouter) -> MethodAwareRouter {
    black_box(matchit_lookup(black_box(&router), black_box("GET"), black_box("/v1/users/octocat")));
    router
}

#[metabench::benchmark(M_DEEP, MATCHIT, "deep")]
#[bench::case(build_matchit())]
fn matchit_deep(router: MethodAwareRouter) -> MethodAwareRouter {
    black_box(matchit_lookup(
        black_box(&router),
        black_box("GET"),
        black_box("/v1/repos/rust-lang/cargo/issues/1347/comments/7"),
    ));
    router
}

#[metabench::benchmark(M_CATCH_ALL, MATCHIT, "catch_all")]
#[bench::case(build_matchit())]
fn matchit_catch_all(router: MethodAwareRouter) -> MethodAwareRouter {
    black_box(matchit_lookup(
        black_box(&router),
        black_box("GET"),
        black_box("/v1/repos/rust-lang/cargo/contents/src/lib/mod.rs"),
    ));
    router
}

#[metabench::benchmark(M_MISS, MATCHIT, "miss")]
#[bench::case(build_matchit())]
fn matchit_miss(router: MethodAwareRouter) -> MethodAwareRouter {
    black_box(matchit_lookup(black_box(&router), black_box("GET"), black_box("/v1/unknown")));
    router
}

fn criterion_benchmarks(c: &mut Criterion) {
    let router = build_matchit();
    for (method, path, expected) in [
        ("GET", "/v1/users/octocat", Some("GetUser")),
        ("GET", "/v1/repos/rust-lang/cargo/issues/1347/comments/7", Some("GetIssueComment")),
        ("GET", "/v1/repos/rust-lang/cargo/contents/src/lib/mod.rs", Some("GetContents")),
        ("GET", "/v1/unknown", None),
    ] {
        let resolved = Route::resolve(method, path);
        assert_eq!(resolved.as_ref().map(rest_over_grpc::codegen_helpers::RouteMatch::name), expected);
        assert_eq!(matchit_lookup(&router, method, path), expected);
    }
    let mut generated = c.benchmark_group(G_SHALLOW.group_name());
    generated.bench_function(G_SHALLOW.benchmark_name(), |b| b.iter(|| black_box(generated_shallow())));
    generated.bench_function(G_DEEP.benchmark_name(), |b| b.iter(|| black_box(generated_deep())));
    generated.bench_function(G_CATCH_ALL.benchmark_name(), |b| b.iter(|| black_box(generated_catch_all())));
    generated.bench_function(G_MISS.benchmark_name(), |b| b.iter(|| black_box(generated_miss())));
    generated.finish();
    let mut matchit = c.benchmark_group(M_SHALLOW.group_name());
    matchit.bench_function(BenchmarkId::new(M_SHALLOW.benchmark_name(), "case"), |b| {
        b.iter(|| black_box(matchit_lookup(black_box(&router), black_box("GET"), black_box("/v1/users/octocat"))));
    });
    matchit.bench_function(BenchmarkId::new(M_DEEP.benchmark_name(), "case"), |b| {
        b.iter(|| {
            black_box(matchit_lookup(
                black_box(&router),
                black_box("GET"),
                black_box("/v1/repos/rust-lang/cargo/issues/1347/comments/7"),
            ))
        });
    });
    matchit.bench_function(BenchmarkId::new(M_CATCH_ALL.benchmark_name(), "case"), |b| {
        b.iter(|| {
            black_box(matchit_lookup(
                black_box(&router),
                black_box("GET"),
                black_box("/v1/repos/rust-lang/cargo/contents/src/lib/mod.rs"),
            ))
        });
    });
    matchit.bench_function(BenchmarkId::new(M_MISS.benchmark_name(), "case"), |b| {
        b.iter(|| black_box(matchit_lookup(black_box(&router), black_box("GET"), black_box("/v1/unknown"))));
    });
    matchit.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [G_SHALLOW, G_DEEP, G_CATCH_ALL, G_MISS, M_SHALLOW, M_DEEP, M_CATCH_ALL, M_MISS]
);
