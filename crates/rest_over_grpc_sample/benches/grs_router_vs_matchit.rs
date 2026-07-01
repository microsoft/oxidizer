// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Algorithm-to-algorithm comparison of the build-time generated static router
//! against a **method-aware** `matchit` setup (the radix-trie path matcher `axum`
//! and others build on).
//!
//! Both are built from the same ~50-route GitHub-like table (`bench_routes.rs`)
//! and driven by the same seeded, shuffled workload of request `(method, path)`
//! pairs — a realistic mix of shallow and deep hits across every resource family
//! plus a fraction of misses.
//!
//! To keep the comparison fair, the `matchit` router is made method-aware exactly
//! as `axum` does — the trie is keyed on the path and each node carries its
//! `(method, rpc)` pairs — so both contenders do the same work: match the path,
//! disambiguate the method, and capture path variables.

#![allow(missing_docs, reason = "Benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use http::Method;
use rest_over_grpc_sample::bench_router::resolve;

// The shared route table (`ROUTES`).
include!("../bench_routes.rs");

/// Converts a `google.api.http` path template into `matchit` 0.8 syntax:
/// `{var}` stays `{var}` (dots in nested names are sanitized to `_`, which does
/// not affect matching), and a trailing `{var=**}` becomes a `{*var}` catch-all.
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
                out.push_str(&sanitize(name));
            } else {
                out.push_str(&sanitize(inner));
            }
            out.push('}');
        } else {
            out.push_str(segment);
        }
    }
    out
}

/// Replaces characters that are invalid in a `matchit` parameter name.
fn sanitize(name: &str) -> String {
    name.replace('.', "_")
}

/// Builds a method-aware `matchit` router, mirroring how `axum` layers method
/// routing on `matchit`: the trie is keyed on the path, and each matched node
/// carries the `(method, rpc)` pairs registered for that path. A lookup therefore
/// does a path match plus a method dispatch — the same work the dynamic router
/// does — so the comparison is apples-to-apples.
///
/// Routes sharing a path (e.g. `GET`/`POST /v1/shelves`) group under one entry,
/// which also sidesteps `matchit`'s rejection of duplicate paths.
fn build_matchit() -> matchit::Router<Vec<(&'static str, &'static str)>> {
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

/// Builds the shuffled request workload: for every route, one concrete matching
/// request, plus a set of misses, shuffled with a fixed seed for reproducibility.
fn build_workload() -> Vec<(Method, String)> {
    let mut requests: Vec<(Method, String)> = Vec::new();

    for (_rpc, method, pattern) in ROUTES {
        let method = Method::from_bytes(method.as_bytes()).expect("valid method");
        requests.push((method, concrete_path(pattern)));
    }

    // A realistic fraction of requests hit nothing.
    for miss in MISSES {
        requests.push((Method::GET, (*miss).to_owned()));
    }

    // Deterministic shuffle so the workload is stable across runs.
    let mut rng = fastrand::Rng::with_seed(0x5eed_1234_9abc_def0);
    rng.shuffle(&mut requests);
    requests
}

/// Substitutes concrete values for the variables in a template to produce a path
/// that matches it.
fn concrete_path(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    for segment in pattern.split('/') {
        if segment.is_empty() {
            continue;
        }
        out.push('/');
        if let Some(inner) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            if inner.ends_with("=**") {
                out.push_str("dir/sub/file.rs");
            } else {
                out.push_str(sample_value(inner));
            }
        } else {
            out.push_str(segment);
        }
    }
    out
}

/// A plausible concrete value for a path variable.
fn sample_value(name: &str) -> &'static str {
    match name {
        "owner" => "rust-lang",
        "repo" => "cargo",
        "user" => "octocat",
        "org" => "github",
        "issue" => "1347",
        "pull" => "42",
        "comment" => "7",
        "branch" => "main",
        "sha" => "a1b2c3d",
        "team" => "core",
        "gist" => "aa11bb22",
        _ => "x",
    }
}

/// Paths that should resolve to no route (misses at various depths).
const MISSES: &[&str] = &[
    "/v1/unknown",
    "/health",
    "/v1/users/octocat/unknownsub",
    "/v1/repos/rust-lang",
    "/v1/repos/rust-lang/cargo/issues/1347/reactions",
    "/v1/orgs/github/billing",
    "/v1/gists/aa11bb22/forks/extra",
    "/v2/users/octocat",
    "/v1/repos/rust-lang/cargo/pulls/42/reviews",
    "/",
];

fn bench_resolve(c: &mut Criterion) {
    let workload = build_workload();
    let matchit_router = build_matchit();

    let mut group = c.benchmark_group("grs_router_vs_matchit/resolve");

    group.bench_function("generated", |b| {
        b.iter(|| {
            for (method, path) in &workload {
                black_box(resolve(black_box(method.as_str()), black_box(path.as_str())));
            }
        });
    });

    group.bench_function("matchit", |b| {
        b.iter(|| {
            for (method, path) in &workload {
                // Path match, then method dispatch on the matched node — the same
                // resolution the generated router performs.
                let hit = matchit_router.at(black_box(path.as_str())).ok().and_then(|m| {
                    m.value
                        .iter()
                        .find(|(candidate, _)| *candidate == method.as_str())
                        .map(|(_, rpc)| *rpc)
                });
                black_box(hit);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_resolve);
criterion_main!(benches);
