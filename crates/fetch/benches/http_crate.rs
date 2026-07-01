// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(clippy::missing_panics_doc, reason = "improves readability in benchmarks")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![expect(missing_docs, reason = "Benchmark code")]

use alloc_tracker::{Allocator, Session};
use criterion::{Criterion, criterion_group, criterion_main};
use http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use http::{HeaderValue, Method, Request};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const URI_STRING: &str = "https://example.com/some/path?query=value";

fn get_uri() -> &'static str {
    URI_STRING
}

fn entry(c: &mut Criterion) {
    let session = Session::new();
    let mut group = c.benchmark_group("http_crate");

    let uri_allocs = session.operation("uri");
    group.bench_function("uri", |b| {
        b.iter(|| {
            let _measure = uri_allocs.measure_thread();
            let _request = Request::builder().method(Method::GET).uri(get_uri()).body(()).unwrap();
        });
    });

    let uri_raw_allocs = session.operation("uri_raw");
    group.bench_function("uri_raw", |b| {
        b.iter(|| {
            let _measure = uri_raw_allocs.measure_thread();
            let _request = Request::builder().method(Method::GET).uri(URI_STRING).body(()).unwrap();
        });
    });

    let single_header_allocs = session.operation("uri_single_header");
    group.bench_function("uri_single_header", |b| {
        b.iter(|| {
            let _measure = single_header_allocs.measure_thread();
            let _request = Request::builder()
                .method(Method::GET)
                .uri(get_uri())
                .header(CONTENT_LENGTH, HeaderValue::from_static("0"))
                .body(())
                .unwrap();
        });
    });

    let two_headers_allocs = session.operation("uri_two_headers");
    group.bench_function("uri_two_headers", |b| {
        b.iter(|| {
            let _measure = two_headers_allocs.measure_thread();
            let _request = Request::builder()
                .method(Method::GET)
                .uri(get_uri())
                .header(CONTENT_LENGTH, HeaderValue::from_static("0"))
                .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
                .body(())
                .unwrap();
        });
    });

    group.finish();
}

criterion_group!(benches, entry);
criterion_main!(benches);
