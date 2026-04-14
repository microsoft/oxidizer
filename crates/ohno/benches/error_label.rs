// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use ohno::ErrorLabel;

fn entry(c: &mut Criterion) {
    let mut group = c.benchmark_group("ErrorLabel");

    // Creation from a static string literal via `from_static`.
    group.bench_function("from_static", |b| {
        b.iter(|| black_box(ErrorLabel::from_static("connection_refused")));
    });

    // Creation from a `&'static str` via the `From` impl (includes coercion check).
    group.bench_function("from_static_str", |b| {
        b.iter(|| black_box(ErrorLabel::from(black_box("connection_refused"))));
    });

    // Creation from a `&'static str` that requires coercion (uppercase → lowercase).
    group.bench_function("from_static_str_coerce", |b| {
        b.iter(|| black_box(ErrorLabel::from(black_box("ConnectionRefused"))));
    });

    // Creation from an owned `String` (valid label, no coercion needed).
    group.bench_function("from_string", |b| {
        b.iter(|| {
            let s = String::from("connection_refused");
            black_box(ErrorLabel::from(black_box(s)))
        });
    });

    // Creation from an owned `String` that requires coercion.
    group.bench_function("from_string_coerce", |b| {
        b.iter(|| {
            let s = String::from("Connection-Refused");
            black_box(ErrorLabel::from(black_box(s)))
        });
    });

    // `from_parts` with two static string labels.
    group.bench_function("from_parts_2", |b| {
        b.iter(|| black_box(ErrorLabel::from_parts(black_box(["http", "timeout"]))));
    });

    // `from_parts` with three static string labels.
    group.bench_function("from_parts_3", |b| {
        b.iter(|| black_box(ErrorLabel::from_parts(black_box(["http", "client", "timeout"]))));
    });

    // `from_parts` with a single label (fast path, no joining needed).
    group.bench_function("from_parts_1", |b| {
        b.iter(|| black_box(ErrorLabel::from_parts(black_box(["timeout"]))));
    });

    // `from_parts` with owned `String` parts.
    group.bench_function("from_parts_owned_2", |b| {
        b.iter(|| {
            let parts = vec![String::from("http"), String::from("timeout")];
            black_box(ErrorLabel::from_parts(black_box(parts)))
        });
    });

    // `as_str` access on an existing label.
    group.bench_function("as_str", |b| {
        let label = ErrorLabel::from_static("connection_refused");
        b.iter(|| black_box(black_box(&label).as_str()));
    });

    // `into_cow` conversion.
    group.bench_function("into_cow", |b| {
        b.iter(|| {
            let label = ErrorLabel::from_static("connection_refused");
            black_box(black_box(label).into_cow())
        });
    });

    // Clone a label.
    group.bench_function("clone", |b| {
        let label = ErrorLabel::from_static("connection_refused");
        b.iter(|| black_box(black_box(&label).clone()));
    });

    group.finish();
}

criterion_group!(benches, entry);
criterion_main!(benches);
