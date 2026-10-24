// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for [`Router::resolve_request_uri`], the per-request routing step.
//!
//! [`Router::resolve_request_uri`] selects the [`BaseUri`] and creates the request
//! `http::Uri` when a caller invokes it.
//! This benchmark measures the initial resolution of a fixed endpoint.
//! It uses a request from [`HttpRequestBuilder`].
//! The builder caches the rendered path.
//!
//! `router_resolve_cg.rs` measures the same operation with Callgrind.
//! Use its instruction count for small differences that wall-clock timing cannot resolve.

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

    // A fixed endpoint with a root base path returns the cached path directly without re-validation.
    let router = Router::fixed(BaseUri::from_static("https://api.example.com"));
    let mut request = built_request("/users/42/posts/hello-world?active=true");

    // resolve_request_uri starts from the preserved original target.
    // Repeated calls therefore measure the same work for each attempt.
    group.bench_function("fixed_root_base", |b| {
        b.iter(|| {
            black_box(&router)
                .resolve_request_uri(RouterContext::new(), black_box(&mut request))
                .unwrap();
        });
    });

    // A fixed endpoint with a non-root base path requires a join and a second URI inspection.
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
