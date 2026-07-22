// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for [`Router::resolve_request_uri`], the per-request routing step.
//!
//! For every outgoing request an HTTP client resolves the target URI against the configured
//! endpoint: it picks the [`BaseUri`] and materializes the request's `http::Uri`. This is run
//! once per request, and again per attempt under retry/hedging. The benchmark exercises it on
//! a request built through [`HttpRequestBuilder`] (so the build-time path rendering is cached
//! and reused) for a fixed base endpoint.
//!
//! Paired with `router_resolve_cg.rs`, which measures the same operation under Callgrind.
//! Instruction counts are the authoritative signal here; wall-clock cannot resolve the small
//! per-attempt differences this path involves.

#![allow(
    clippy::missing_panics_doc,
    clippy::unwrap_used,
    missing_docs,
    reason = "improves readability in benchmarks"
)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use http_extensions::routing::{Router, RouterContext};
use http_extensions::{HttpRequest, HttpRequestBuilder};
use templated_uri::BaseUri;

fn built_request(path: &'static str) -> HttpRequest {
    HttpRequestBuilder::new_fake().get(path).build().unwrap()
}

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("router_resolve");

    // A fixed endpoint with a root base path (the common case): the reused rendered path is
    // returned directly without a re-validation scan.
    let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
    let mut request = built_request("/users/42/posts/hello-world?active=true");

    // `resolve_request_uri` re-routes idempotently from the request's preserved original
    // target, so repeated calls do identical work - exactly the per-attempt cost.
    group.bench_function("fixed_root_base", |b| {
        b.iter(|| {
            black_box(&router)
                .resolve_request_uri(RouterContext::new(), black_box(&mut request))
                .unwrap();
        });
    });

    // A fixed endpoint with a non-root base path: the join must concatenate and re-validate.
    let router_prefixed = Router::fixed(BaseUri::from_static("https://api.example.com/v1/"));
    let mut request_prefixed = built_request("/users/42/posts/hello-world?active=true");

    group.bench_function("fixed_prefixed_base", |b| {
        b.iter(|| {
            black_box(&router_prefixed)
                .resolve_request_uri(RouterContext::new(), black_box(&mut request_prefixed))
                .unwrap();
        });
    });

    group.finish();
}

criterion_group!(benches, entry);
criterion_main!(benches);
