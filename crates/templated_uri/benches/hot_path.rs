// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for the per-request URI hot path in `templated_uri`.
//!
//! An HTTP client builds a URI for every outgoing request: for a dynamic REST
//! path it escapes variable values, renders a templated path, assembles a
//! `Uri` (the base is configured once and reused), and materializes the full
//! `http::Uri`. These benchmarks measure each of those per-call steps.
//!
//! Paired with `hot_path_cg.rs`, which measures the same operations under
//! Callgrind instruction counts.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use templated_uri::{BaseUri, EscapedString, PathAndQueryTemplate as _, Uri, templated};

// A realistic REST path: a numeric id plus an escaped string segment.
#[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
#[derive(Clone)]
struct UserPostPath {
    user_id: u32,
    post_id: EscapedString,
}

// A dynamic value carrying reserved characters, so `escape` must percent-encode.
const RAW_VALUE: &str = "my post title/with?reserved=chars";

// A dynamic value that is already URI-clean, exercising the no-encoding fast path
// where `escape` must avoid touching the allocator beyond owning the input.
const CLEAN_VALUE: &str = "already-clean_value.42~ok";

fn sample_path() -> UserPostPath {
    UserPostPath {
        user_id: 42,
        // Owned value, mirroring a real request where the segment was just escaped.
        post_id: EscapedString::escape(String::from("hello-world")),
    }
}

fn sample_base() -> BaseUri {
    BaseUri::from_static("https://api.example.com")
}

fn hot_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("hot_path");

    // Percent-encode a dynamic string value that contains reserved characters.
    group.bench_function("escape", |b| b.iter(|| black_box(EscapedString::escape(black_box(RAW_VALUE)))));

    // Escape a value that needs no encoding: exercises the allocation-free scan.
    group.bench_function("escape_clean", |b| {
        b.iter(|| black_box(EscapedString::escape(black_box(CLEAN_VALUE))));
    });

    let path = sample_path();

    // Render a templated path into a `String`.
    group.bench_function("render", |b| b.iter(|| black_box(black_box(&path).render())));

    // Render + validate into an `http` `PathAndQuery`.
    group.bench_function("to_path_and_query", |b| {
        b.iter(|| black_box(black_box(&path).to_path_and_query().expect("valid path-and-query")));
    });

    // End-to-end per-call construction: a reused base plus a freshly built path.
    let base = sample_base();
    group.bench_function("build_uri", |b| {
        b.iter(|| {
            black_box(
                Uri::default()
                    .with_base(black_box(&base).clone())
                    .with_path_and_query(black_box(&path).clone()),
            )
        });
    });

    // Full per-request materialization: build the `Uri` (reused base + fresh templated
    // path) and convert it into a validated `http::Uri`, exactly as an HTTP client does
    // for every outgoing request. This is the real hot path end to end.
    group.bench_function("materialize", |b| {
        b.iter(|| {
            let uri = Uri::default()
                .with_base(black_box(&base).clone())
                .with_path_and_query(black_box(&path).clone());
            black_box(http::Uri::try_from(uri).expect("valid http uri"))
        });
    });

    group.finish();
}

criterion_group!(benches, hot_path);
criterion_main!(benches);
