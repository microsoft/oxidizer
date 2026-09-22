// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed token reads versus explicit owned-token materialization.

#![expect(clippy::unwrap_used, reason = "benchmark fixtures and validated token conversions must succeed")]

use std::hint::black_box;
use std::sync::OnceLock;

use criterion::Criterion;
use http::{HeaderMap, HeaderValue, Method};
use http_headers::FieldName;
use http_headers::headers::{Allow, MethodView, Vary};

fn allow_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HeaderMap::new();
        map.insert("allow", HeaderValue::from_static("GET, HEAD, POST, CUSTOM, PURGE"));
        map
    })
}

fn vary_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HeaderMap::new();
        map.insert("vary", HeaderValue::from_static("Accept-Encoding, X-Tenant, X-Region"));
        map
    })
}

#[metabench::benchmark(ALLOW_TYPED, "http_headers_typed_tokens/reads", "allow_typed", gungraun_setup = allow_map)]
fn allow_typed(map: &'static HeaderMap) -> bool {
    let value = Allow::view(black_box(map)).unwrap().unwrap();
    let query = MethodView::new(black_box("PURGE")).unwrap();
    black_box(value.methods().any(|method| method == query))
}

#[metabench::benchmark(ALLOW_MATERIALIZED, "http_headers_typed_tokens/reads", "allow_materialized", gungraun_setup = allow_map)]
fn allow_materialized(map: &'static HeaderMap) -> bool {
    let value = Allow::view(black_box(map)).unwrap().unwrap();
    let query = Method::from_bytes(black_box(b"PURGE")).unwrap();
    black_box(value.items().any(|method| Method::from_bytes(method).unwrap() == query))
}

#[metabench::benchmark(ALLOW_TYPED_8, "http_headers_typed_tokens/reads", "allow_typed_8", gungraun_setup = allow_map)]
fn allow_typed_8(map: &'static HeaderMap) -> usize {
    let value = Allow::view(black_box(map)).unwrap().unwrap();
    let query = MethodView::new("PURGE").unwrap();
    (0..8)
        .map(|_| usize::from(black_box(value.methods().any(|method| method == black_box(query)))))
        .sum()
}

#[metabench::benchmark(ALLOW_MATERIALIZED_8, "http_headers_typed_tokens/reads", "allow_materialized_8", gungraun_setup = allow_map)]
fn allow_materialized_8(map: &'static HeaderMap) -> usize {
    let value = Allow::view(black_box(map)).unwrap().unwrap();
    let query = Method::from_bytes(b"PURGE").unwrap();
    (0..8)
        .map(|_| {
            usize::from(black_box(
                value
                    .items()
                    .any(|method| Method::from_bytes(method).unwrap() == *black_box(&query)),
            ))
        })
        .sum()
}

#[metabench::benchmark(VARY_TYPED, "http_headers_typed_tokens/reads", "vary_typed", gungraun_setup = vary_map)]
fn vary_typed(map: &'static HeaderMap) -> bool {
    let value = Vary::view(black_box(map)).unwrap().unwrap();
    black_box(value.entries().any(|entry| {
        entry.is_wildcard()
            || entry
                .field_name()
                .is_some_and(|name| name.eq_ignore_ascii_case(black_box("x-region")))
    }))
}

#[metabench::benchmark(VARY_MATERIALIZED, "http_headers_typed_tokens/reads", "vary_materialized", gungraun_setup = vary_map)]
fn vary_materialized(map: &'static HeaderMap) -> bool {
    let value = Vary::view(black_box(map)).unwrap().unwrap();
    black_box(value.items().any(|name| {
        name == b"*"
            || FieldName::try_from_bytes(name)
                .unwrap()
                .as_str()
                .eq_ignore_ascii_case(black_box("x-region"))
    }))
}

#[metabench::benchmark(VARY_TYPED_8, "http_headers_typed_tokens/reads", "vary_typed_8", gungraun_setup = vary_map)]
fn vary_typed_8(map: &'static HeaderMap) -> usize {
    let value = Vary::view(black_box(map)).unwrap().unwrap();
    (0..8)
        .map(|_| {
            usize::from(black_box(value.entries().any(|entry| {
                entry.is_wildcard()
                    || entry
                        .field_name()
                        .is_some_and(|name| name.eq_ignore_ascii_case(black_box("x-region")))
            })))
        })
        .sum()
}

#[metabench::benchmark(VARY_MATERIALIZED_8, "http_headers_typed_tokens/reads", "vary_materialized_8", gungraun_setup = vary_map)]
fn vary_materialized_8(map: &'static HeaderMap) -> usize {
    let value = Vary::view(black_box(map)).unwrap().unwrap();
    (0..8)
        .map(|_| {
            usize::from(black_box(value.items().any(|name| {
                name == b"*"
                    || FieldName::try_from_bytes(name)
                        .unwrap()
                        .as_str()
                        .eq_ignore_ascii_case(black_box("x-region"))
            })))
        })
        .sum()
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ALLOW_TYPED.group_name());
    macro_rules! register {
        ($id:ident, $function:ident, $setup:ident) => {
            group.bench_function($id.benchmark_name(), |bencher| {
                let input = $setup();
                bencher.iter(|| $function(input));
            });
        };
    }
    register!(ALLOW_TYPED, allow_typed, allow_map);
    register!(ALLOW_MATERIALIZED, allow_materialized, allow_map);
    register!(ALLOW_TYPED_8, allow_typed_8, allow_map);
    register!(ALLOW_MATERIALIZED_8, allow_materialized_8, allow_map);
    register!(VARY_TYPED, vary_typed, vary_map);
    register!(VARY_MATERIALIZED, vary_materialized, vary_map);
    register!(VARY_TYPED_8, vary_typed_8, vary_map);
    register!(VARY_MATERIALIZED_8, vary_materialized_8, vary_map);
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        ALLOW_TYPED,
        ALLOW_MATERIALIZED,
        ALLOW_TYPED_8,
        ALLOW_MATERIALIZED_8,
        VARY_TYPED,
        VARY_MATERIALIZED,
        VARY_TYPED_8,
        VARY_MATERIALIZED_8,
    ],
);
