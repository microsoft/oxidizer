// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wall-clock benchmarks for [`PathTemplate::parse`].
//!
//! Parsing a `google.api.http` path template is this crate's entire value
//! proposition: a REST-over-gRPC build step parses every annotated method's
//! template once, and a router may re-parse templates when (re)building its
//! route table. Each parse walks the string, splits it into segments, and
//! allocates the resulting [`Segment`](http_path_template::Segment) AST.
//!
//! These benchmarks isolate a single `parse` call across the grammar's
//! branching shapes — literal-only paths, `{variable}` bindings, a `**` rest
//! wildcard, a trailing `:verb`, an extended-grammar intra-segment affix, and
//! the error path — so a change in per-shape parse cost is visible.
//!
//! Paired with `hpt_parse_cg.rs`, which measures the same operations under
//! Callgrind. Wall-clock cannot reliably resolve a single eliminated
//! allocation or branch; the Callgrind instruction counts are the authoritative
//! signal for those.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use http_path_template::{Grammar, PathTemplate};

// A path made only of literal segments — no variables, no wildcards.
const LITERAL_ONLY: &str = "/v1/shelves/books/list";

// A typical CRUD annotation with two `{variable}` bindings.
const VARIABLES: &str = "/v1/shelves/{shelf}/books/{book}";

// A variable whose sub-template ends in a `**` rest wildcard.
const REST_WILDCARD: &str = "/v1/{name=books/**}";

// A path with two variables and a trailing custom `:verb`.
const VERB: &str = "/v1/shelves/{shelf}/books/{book}:archive";

// An extended-grammar intra-segment affix (`{name}` wrapped in `files/` … `.json`).
const AFFIX: &str = "/v1/files/{name}.json";

// An invalid template (`**` is not the final segment) — exercises the error path.
const INVALID: &str = "/a/**/b";

fn parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("hpt_parse/parse");

    let strict = Grammar::default();
    let extended = Grammar::default().with_segment_affixes();

    group.bench_function("literal_only", |b| {
        b.iter(|| black_box(PathTemplate::parse(black_box(LITERAL_ONLY), strict)));
    });
    group.bench_function("variables", |b| {
        b.iter(|| black_box(PathTemplate::parse(black_box(VARIABLES), strict)));
    });
    group.bench_function("rest_wildcard", |b| {
        b.iter(|| black_box(PathTemplate::parse(black_box(REST_WILDCARD), strict)));
    });
    group.bench_function("verb", |b| {
        b.iter(|| black_box(PathTemplate::parse(black_box(VERB), strict)));
    });
    group.bench_function("affix", |b| {
        b.iter(|| black_box(PathTemplate::parse(black_box(AFFIX), extended)));
    });
    group.bench_function("invalid", |b| {
        b.iter(|| black_box(PathTemplate::parse(black_box(INVALID), strict)));
    });

    group.finish();
}

criterion_group!(benches, parse);
criterion_main!(benches);
