// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for [`EscapedString`] construction.
//!
//! An HTTP client escapes every dynamic string URI component (path segment or query value)
//! for each outgoing request: an id, a slug, a name, a short token. `EscapedString::escape`
//! owns the escaped bytes, so today it heap-allocates for every such value.
//!
//! These benchmarks isolate that construction across the range of value lengths that matter
//! for a small-string optimization: short values that could live inline (<= 24 bytes on
//! 64-bit) versus longer values that always spill to the heap. A per-request group builds a
//! templated struct with several short escaped fields, mirroring the real per-call pattern.
//!
//! Paired with `escaped_string_cg.rs`, which measures the same operations under Callgrind.
//! Wall-clock cannot reliably resolve a single eliminated `malloc`; the Callgrind
//! instruction counts are the authoritative signal for allocation changes.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use templated_uri::{EscapedString, templated};

// A short, already-clean value: no encoding, and short enough to live inline once
// `EscapedString` uses a small-string-optimized backing store.
const SHORT_CLEAN: &str = "hello-world";

// A short value that must be percent-encoded; its escaped form is still short enough to
// live inline.
const SHORT_ENCODED_INPUT: &str = "a b/c?d"; // -> "a%20b%2Fc%3Fd" (13 bytes)

// Exactly 24 bytes (clean): the largest value that fits inline on 64-bit.
const BOUNDARY_INLINE: &str = "abcdefghijklmnopqrstuvwx";

// 25 bytes (clean): one byte over the inline capacity, so it always spills to the heap.
const BOUNDARY_HEAP: &str = "abcdefghijklmnopqrstuvwxy";

// A long clean value that always lives on the heap.
const LONG_CLEAN: &str = "abcdefghijklmnopqrstuvwxyz0123456789-._~ABCDEFG";

// A long value needing encoding whose escaped form always lives on the heap.
const LONG_ENCODED_INPUT: &str = "my post title/with?reserved=chars&and=more stuff";

// A realistic templated path with several short, escaped string components.
#[templated(template = "/orgs/{org}/users/{user}/posts/{post}", unredacted)]
#[derive(Clone)]
struct RequestPath {
    org: EscapedString,
    user: EscapedString,
    post: EscapedString,
}

fn construct(c: &mut Criterion) {
    let mut group = c.benchmark_group("escaped_construct");

    group.bench_function("short_clean", |b| {
        b.iter(|| black_box(EscapedString::escape(black_box(SHORT_CLEAN))));
    });
    group.bench_function("short_encoded", |b| {
        b.iter(|| black_box(EscapedString::escape(black_box(SHORT_ENCODED_INPUT))));
    });
    group.bench_function("boundary_inline_24", |b| {
        b.iter(|| black_box(EscapedString::escape(black_box(BOUNDARY_INLINE))));
    });
    group.bench_function("boundary_heap_25", |b| {
        b.iter(|| black_box(EscapedString::escape(black_box(BOUNDARY_HEAP))));
    });
    group.bench_function("long_clean", |b| {
        b.iter(|| black_box(EscapedString::escape(black_box(LONG_CLEAN))));
    });
    group.bench_function("long_encoded", |b| {
        b.iter(|| black_box(EscapedString::escape(black_box(LONG_ENCODED_INPUT))));
    });

    group.finish();
}

fn request(c: &mut Criterion) {
    let mut group = c.benchmark_group("request_construct");

    // Build a templated struct from several short escaped components, as a client does for
    // every outgoing request. Each `escape` owns its value, so this is where the per-request
    // string allocations are incurred.
    group.bench_function("three_short_fields", |b| {
        b.iter(|| {
            black_box(RequestPath {
                org: EscapedString::escape(black_box("contoso")),
                user: EscapedString::escape(black_box("Will_E_Coyote")),
                post: EscapedString::escape(black_box("hello-world")),
            })
        });
    });

    group.finish();
}

criterion_group!(benches, construct, request);
criterion_main!(benches);
