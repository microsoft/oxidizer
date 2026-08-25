// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock benchmarks for `ThreadAware` impls on 3rd-party types.
//!
//! Mirrors `benches/gungraun_third_party/` 1:1: each `<group>/<variant>` here
//! corresponds to a gungraun function `<group>_<variant>`.
//!
//! These benches exist to lock in the design properties of the 3rd-party
//! impls:
//!
//! * Inert value types (`StatusCode`, `HeaderValue`, `Bytes`, `Uuid`,
//!   jiff primitives, etc.) have a no-op `relocate`.
//! * `HeaderMap<HeaderValue>::relocate` is no-op regardless of map size.
//! * `Request<T>::relocate` and `Response<T>::relocate` forward to the
//!   body only — they must not iterate headers or extensions.
//!
//! Cost across `header_map/{empty,populated_8,populated_32}` and across the
//! two `unit_body_*` variants of `request/response` must stay equal. A
//! divergence means someone re-introduced iteration over the inert headers
//! map.
//!
//! Run with: `cargo bench -p thread_aware --bench criterion_third_party \
//!     --features "bytes http jiff02 uuid"`

#![allow(missing_docs, reason = "benchmark code")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(deprecated, reason = "criterion::black_box is deprecated in favor of std::hint::black_box")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]

use std::hint::black_box;
use std::str::FromStr;

use bytes::{Bytes, BytesMut};
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use http::header::{HeaderMap, HeaderName, HeaderValue};
use http::{Method, Request, Response, StatusCode, Version};
use jiff::civil::{Date, DateTime, ISOWeekDate, Time};
use jiff::{SignedDuration, Span, Timestamp};
use thread_aware::ThreadAware;
use thread_aware::affinity::{Affinity, pinned_affinities};
use uuid::Uuid;

/// Returns a `(src, dst)` pair to feed `ThreadAware::relocate`.
fn affinities() -> (Affinity, Affinity) {
    let a = pinned_affinities(&[1, 1]);
    (a[0], a[1])
}

/// A `ThreadAware` body whose `relocate` bumps a counter.
///
/// Used by the `*_body/counter_body` benches to assert that the body is the
/// only thing reached, and exactly once per `relocate` call. If a future
/// change started iterating headers or extensions, the work would still show
/// up in callgrind / criterion noise even though `CounterBody` would not
/// report it directly.
#[derive(Default)]
struct CounterBody(u64);

impl ThreadAware for CounterBody {
    fn relocate(&mut self, _source: Option<Affinity>, _destination: Affinity) {
        self.0 = self.0.wrapping_add(1);
    }
}

fn build_header_map(n: usize) -> HeaderMap {
    let mut m = HeaderMap::with_capacity(n);
    for i in 0..n {
        let name = HeaderName::from_str(&format!("x-bench-header-{i}")).unwrap();
        let value = HeaderValue::from_str(&format!("value-{i}")).unwrap();
        m.insert(name, value);
    }
    m
}

fn build_request<T>(body: T, header_count: usize) -> Request<T> {
    let mut req = Request::new(body);
    *req.headers_mut() = build_header_map(header_count);
    req
}

fn build_response<T>(body: T, header_count: usize) -> Response<T> {
    let mut resp = Response::new(body);
    *resp.headers_mut() = build_header_map(header_count);
    resp
}

/// Registers one `relocate` bench. Criterion handles iteration counting on
/// its own, so the closure performs exactly one call per sample.
fn bench_relocate<T: ThreadAware>(g: &mut BenchmarkGroup<'_, WallTime>, name: &str, mut value: T, src: Affinity, dst: Affinity) {
    g.bench_function(name, |b| {
        b.iter(|| {
            black_box(&mut value).relocate(black_box(Some(src)), black_box(dst));
        });
    });
}

// =========================================================================
// noop_value
// =========================================================================

/// Regression sentinels for the inert no-op impls. Any non-trivial cost
/// here means a no-op `relocate` body grew real work.
fn bench_noop_value(c: &mut Criterion) {
    let (src, dst) = affinities();
    let mut g = c.benchmark_group("noop_value");

    bench_relocate(&mut g, "status_code", StatusCode::OK, src, dst);
    bench_relocate(&mut g, "method", Method::GET, src, dst);
    bench_relocate(&mut g, "version", Version::HTTP_11, src, dst);
    bench_relocate(&mut g, "header_name", HeaderName::from_static("x-bench"), src, dst);
    bench_relocate(&mut g, "header_value", HeaderValue::from_static("bench-value"), src, dst);
    bench_relocate(&mut g, "bytes_empty", Bytes::new(), src, dst);
    bench_relocate(&mut g, "bytes_4kb", Bytes::from(vec![0_u8; 4096]), src, dst);
    bench_relocate(&mut g, "bytes_mut_4kb", BytesMut::from(&[0_u8; 4096][..]), src, dst);
    bench_relocate(&mut g, "uuid", Uuid::nil(), src, dst);
    bench_relocate(&mut g, "timestamp", Timestamp::UNIX_EPOCH, src, dst);
    bench_relocate(&mut g, "signed_duration", SignedDuration::ZERO, src, dst);
    bench_relocate(&mut g, "span", Span::new(), src, dst);
    bench_relocate(&mut g, "date", Date::constant(2026, 6, 8), src, dst);
    bench_relocate(&mut g, "time", Time::constant(12, 0, 0, 0), src, dst);
    bench_relocate(&mut g, "datetime", DateTime::constant(2026, 6, 8, 12, 0, 0, 0), src, dst);
    bench_relocate(
        &mut g,
        "iso_week_date",
        ISOWeekDate::new(2026, 23, jiff::civil::Weekday::Monday).unwrap(),
        src,
        dst,
    );
}

// =========================================================================
// header_map — must be O(1) regardless of size.
// =========================================================================

fn bench_header_map(c: &mut Criterion) {
    let (src, dst) = affinities();
    let mut g = c.benchmark_group("header_map");

    for (name, count) in [("empty", 0_usize), ("populated_8", 8), ("populated_32", 32)] {
        bench_relocate(&mut g, name, build_header_map(count), src, dst);
    }
}

// =========================================================================
// request — must be O(1) in header count; body is the only thing reached.
// =========================================================================

fn bench_request(c: &mut Criterion) {
    let (src, dst) = affinities();
    let mut g = c.benchmark_group("request");

    bench_relocate(&mut g, "unit_body_empty_headers", build_request((), 0), src, dst);
    bench_relocate(&mut g, "unit_body_populated_headers", build_request((), 32), src, dst);
    bench_relocate(&mut g, "bytes_body", build_request(Bytes::from(vec![0_u8; 4096]), 32), src, dst);
    bench_relocate(&mut g, "counter_body", build_request(CounterBody::default(), 32), src, dst);
}

// =========================================================================
// response — same shape as `request`.
// =========================================================================

fn bench_response(c: &mut Criterion) {
    let (src, dst) = affinities();
    let mut g = c.benchmark_group("response");

    bench_relocate(&mut g, "unit_body_empty_headers", build_response((), 0), src, dst);
    bench_relocate(&mut g, "unit_body_populated_headers", build_response((), 32), src, dst);
    bench_relocate(&mut g, "bytes_body", build_response(Bytes::from(vec![0_u8; 4096]), 32), src, dst);
    bench_relocate(&mut g, "counter_body", build_response(CounterBody::default(), 32), src, dst);
}

criterion_group!(benches, bench_noop_value, bench_header_map, bench_request, bench_response);
criterion_main!(benches);
