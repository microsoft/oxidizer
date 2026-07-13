// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks demonstrating the *single-materialization* optimization on the fetch request
//! hot path.
//!
//! Before this optimization an HTTP client materialized the outgoing `http::Uri` twice for
//! every request that goes through routing:
//!
//! 1. `HttpRequestBuilder::build()` renders the templated path and materializes it (so the
//!    `http::Request` has a valid URI and to back the `PathAndQuery` extension), then
//! 2. `Router::resolve_request_uri` re-renders the *same* path from the typed `Uri` while
//!    joining it onto the reconciled base, throwing the first result away.
//!
//! Routing never changes the resource - it only swaps the base - so the second template
//! render is redundant. The optimization has `build()` stash its already-rendered (and
//! validated) `http::PathAndQuery` so routing reuses it, joining only the base.
//!
//! These benchmarks model that difference using only public API:
//!
//! - the *pre-optimization* routing step re-materializes from a **templated** path (which
//!   re-renders and re-escapes the parameters), whereas
//! - the *optimized* routing step (via [`Uri::to_http_uri`]) reuses a **pre-rendered** path,
//!   joining only the base onto the bytes `build()` already produced.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use templated_uri::{BaseUri, EscapedString, Uri, templated};

// A realistic REST path: a numeric id plus an escaped string segment.
#[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
#[derive(Clone)]
struct UserPostPath {
    user_id: u32,
    post_id: EscapedString,
}

fn sample_path() -> UserPostPath {
    UserPostPath {
        user_id: 42,
        post_id: EscapedString::escape(String::from("hello-world")),
    }
}

fn sample_base() -> BaseUri {
    BaseUri::from_static("https://api.example.com")
}

// A heavier path: many literal segments plus several parameters and a multi-key query. The
// per-request *render* cost scales with template size, so the redundant second render this
// optimization removes is a larger share here than for a short path.
#[templated(
    template = "/orgs/{org}/teams/{team}/projects/{project}/users/{user}/posts/{post}{?sort,order,limit,offset}",
    unredacted
)]
#[derive(Clone)]
struct HeavyPath {
    org: EscapedString,
    team: EscapedString,
    project: EscapedString,
    user: u32,
    post: u32,
    sort: EscapedString,
    order: EscapedString,
    limit: u32,
    offset: u32,
}

fn sample_heavy_path() -> HeavyPath {
    HeavyPath {
        org: EscapedString::escape(String::from("contoso")),
        team: EscapedString::escape(String::from("platform")),
        project: EscapedString::escape(String::from("oxidizer")),
        user: 42,
        post: 1001,
        sort: EscapedString::escape(String::from("created")),
        order: EscapedString::escape(String::from("desc")),
        limit: 50,
        offset: 100,
    }
}

/// Renders a templated path once into a static `http::PathAndQuery`, modelling the bytes
/// that `build()` produces and that routing reuses.
fn prerendered<P: templated_uri::PathAndQueryTemplate>(path: &P) -> http::uri::PathAndQuery {
    http::uri::PathAndQuery::try_from(path.render()).expect("rendered path is a valid path-and-query")
}

fn bench(c: &mut Criterion) {
    bench_route_materialize(c);
    bench_per_send(c);
    bench_per_send_hedged(c);
}

// The single step the optimization changes: how routing produces the request `http::Uri`.
// `rerender_current` re-renders the template (`http::Uri::try_from`, the pre-optimization
// behavior); `reuse_cached_optimized` calls the implemented reuse API
// (`Uri::to_http_uri`), joining only the base onto the path rendered once at
// build time. Measured for a short and a heavier path, since the removed render scales with
// template size.
fn bench_route_materialize(c: &mut Criterion) {
    let base = sample_base();
    let path = sample_path();
    let heavy = sample_heavy_path();

    // The typed `Uri` the router resolves to (base + templated path), plus the standalone
    // path rendering that `build()` cached for reuse.
    let uri = Uri::default().with_base(base.clone()).with_path_and_query(path.clone());
    let rendered = prerendered(&path);
    let heavy_uri = Uri::default().with_base(base).with_path_and_query(heavy.clone());
    let heavy_rendered = prerendered(&heavy);

    let mut group = c.benchmark_group("route_materialize");

    group.bench_function("rerender_current", |b| {
        b.iter(|| black_box(http::Uri::try_from(black_box(&uri).clone()).expect("valid http uri")));
    });

    group.bench_function("reuse_cached_optimized", |b| {
        b.iter(|| black_box(black_box(&uri).to_http_uri(Some(black_box(&rendered))).expect("valid http uri")));
    });

    group.bench_function("rerender_current_heavy", |b| {
        b.iter(|| black_box(http::Uri::try_from(black_box(&heavy_uri).clone()).expect("valid http uri")));
    });

    group.bench_function("reuse_cached_optimized_heavy", |b| {
        b.iter(|| {
            black_box(
                black_box(&heavy_uri)
                    .to_http_uri(Some(black_box(&heavy_rendered)))
                    .expect("valid http uri"),
            )
        });
    });

    group.finish();
}

// End-to-end per request (single attempt): `build()` renders once (identical on both sides),
// then routing runs. Current re-renders the template at routing; optimized reuses the bytes
// `build()` already produced.
fn bench_per_send(c: &mut Criterion) {
    let base = sample_base();
    let path = sample_path();

    let mut group = c.benchmark_group("per_send");

    group.bench_function("double_render_current", |b| {
        b.iter(|| {
            // build(): render the path once into the request's path-and-query.
            let built = prerendered(black_box(&path));
            black_box(&built);
            // routing: re-render the same template while joining the base (build's render is
            // not reused today).
            let routed = http::Uri::try_from(
                Uri::default()
                    .with_base(black_box(&base).clone())
                    .with_path_and_query(black_box(&path).clone()),
            )
            .expect("valid http uri");
            black_box((built, routed))
        });
    });

    group.bench_function("single_render_optimized", |b| {
        // The typed `Uri` stays templated, exactly as in production routing; the separately
        // rendered path is reused via `Uri::to_http_uri`.
        let uri = Uri::default().with_base(base.clone()).with_path_and_query(path.clone());
        b.iter(|| {
            // build(): render the path once into a reusable http path-and-query.
            let built = prerendered(black_box(&path));
            // routing: reuse the cached rendering via the production reuse API (no re-render).
            let routed = black_box(&uri).to_http_uri(Some(black_box(&built))).expect("valid http uri");
            black_box(routed)
        });
    });

    group.finish();
}

// Retry/hedging amplifies the win: routing re-resolves once per attempt, so the current path
// re-renders the template N times while the optimized path renders once (at build) and reuses
// it for every attempt.
fn bench_per_send_hedged(c: &mut Criterion) {
    const ATTEMPTS: usize = 3;

    let base = sample_base();
    let path = sample_path();

    let mut group = c.benchmark_group("per_send_hedged_x3");

    group.bench_function("rerender_each_attempt_current", |b| {
        b.iter(|| {
            let built = prerendered(black_box(&path));
            black_box(&built);
            for _ in 0..ATTEMPTS {
                let routed = http::Uri::try_from(
                    Uri::default()
                        .with_base(black_box(&base).clone())
                        .with_path_and_query(black_box(&path).clone()),
                )
                .expect("valid http uri");
                black_box(routed);
            }
        });
    });

    group.bench_function("reuse_cached_each_attempt_optimized", |b| {
        // Templated typed `Uri` plus a rendering reused across every attempt via `to_http_uri`.
        let uri = Uri::default().with_base(base.clone()).with_path_and_query(path.clone());
        b.iter(|| {
            let built = prerendered(black_box(&path));
            for _ in 0..ATTEMPTS {
                let routed = black_box(&uri).to_http_uri(Some(black_box(&built))).expect("valid http uri");
                black_box(routed);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
