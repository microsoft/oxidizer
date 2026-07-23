// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]

use criterion::{Criterion, criterion_group, criterion_main};

include!("common/header_cache_scenarios.rs");

fn header_cache(c: &mut Criterion) {
    let headers = date_headers();
    let (mut warm_cache, cached_headers) = warm_date_cache();
    let mut date = c.benchmark_group("routerama_header_cache/date");
    date.bench_function("uncached", |b| b.iter(|| date_uncached(black_box(&headers))));
    date.bench_function("cached", |b| b.iter(|| date_cached(&mut warm_cache, &cached_headers)));
    date.finish();

    let headers = accept_encoding_headers();
    let (mut warm_cache, cached_headers) = warm_accept_encoding_cache();
    let mut encoding = c.benchmark_group("routerama_header_cache/accept_encoding");
    encoding.bench_function("uncached", |b| b.iter(|| accept_encoding_uncached(black_box(&headers))));
    encoding.bench_function("cached", |b| b.iter(|| accept_encoding_cached(&mut warm_cache, &cached_headers)));
    encoding.finish();
}

criterion_group!(benches, header_cache);
criterion_main!(benches);
