// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unified `http_headers` benchmarks for field-name recognition and `http` name conversion.
//!
//! Every other benchmark in this crate starts from a `&'static FieldName`
//! constant or a prebuilt map, so the recognition path that turns wire bytes
//! into a [`FieldName`] is invisible to them. This file measures it directly:
//!
//! * `parse_known_names` — the field-name set of an ordinary browser request,
//!   in wire case, across the whole length range. Every name is a hit.
//! * `parse_custom_names_lowercase` / `parse_custom_names_mixed_case` —
//!   matched vendor and tracing names no table entry recognizes, separating
//!   already-normalized input from HTTP/1-style normalization.
//! * `custom_name_index` — the dense-index lookup a `Custom` name performs,
//!   which recognizes the name a second time.
//! * `http_names_into_crate_names` / `crate_names_into_http_names` — the bulk
//!   conversion an adapter performs when it moves a whole `http::HeaderMap`
//!   across the crate boundary.
//!
//! Setup builds the corpora outside the measured region, so each engine
//! attributes a case to recognition and nothing else. Each case consumes its
//! result, so a shortcut that skipped the work fails instead of posting a
//! better number.

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion};
use http_headers::FieldName;

const RECOGNITION: &str = "http_headers_name_recognition/recognition";
const HTTP_CONVERSION: &str = "http_headers_name_recognition/http_conversion";

/// The field names of an ordinary browser request, in wire case.
///
/// The set spans the length range of the well-known table, from the shortest
/// entry to one of the longest, so no case is confined to one length bucket.
const KNOWN_NAMES: &[&[u8]] = &[
    b"Host",
    b"User-Agent",
    b"Accept",
    b"Accept-Language",
    b"Accept-Encoding",
    b"Connection",
    b"Cookie",
    b"Referer",
    b"Cache-Control",
    b"Content-Type",
    b"Content-Length",
    b"Authorization",
    b"TE",
    b"Sec-WebSocket-Key",
    b"Access-Control-Request-Headers",
];

/// Vendor and tracing names no well-known entry matches.
const CUSTOM_NAMES_LOWERCASE: &[&[u8]] = &[
    b"x-request-id",
    b"x-forwarded-for",
    b"x-forwarded-proto",
    b"x-trace-id",
    b"cf-ray",
    b"x-amzn-trace-id",
    b"x-correlation-id",
    b"x-real-ip",
];

const CUSTOM_NAMES_MIXED_CASE: &[&[u8]] = &[
    b"X-Request-Id",
    b"X-Forwarded-For",
    b"X-Forwarded-Proto",
    b"X-Trace-Id",
    b"CF-Ray",
    b"X-Amzn-Trace-Id",
    b"X-Correlation-Id",
    b"X-Real-Ip",
];

fn known_names() -> &'static [&'static [u8]] {
    KNOWN_NAMES
}

fn custom_names_lowercase() -> &'static [&'static [u8]] {
    assert_eq!(CUSTOM_NAMES_LOWERCASE.len(), CUSTOM_NAMES_MIXED_CASE.len());
    assert!(
        CUSTOM_NAMES_LOWERCASE
            .iter()
            .zip(CUSTOM_NAMES_MIXED_CASE)
            .all(|(lowercase, mixed_case)| lowercase.eq_ignore_ascii_case(mixed_case))
    );
    CUSTOM_NAMES_LOWERCASE
}

fn custom_names_mixed_case() -> &'static [&'static [u8]] {
    CUSTOM_NAMES_MIXED_CASE
}

fn custom_header_names() -> &'static [FieldName] {
    CUSTOM_NAMES_MIXED_CASE
        .iter()
        .map(|name| FieldName::try_from_bytes(name).expect("valid field name"))
        .collect::<Vec<_>>()
        .leak()
}

fn http_names() -> &'static [http::HeaderName] {
    KNOWN_NAMES
        .iter()
        .chain(CUSTOM_NAMES_MIXED_CASE)
        .map(|name| http::HeaderName::from_bytes(name).expect("valid field name"))
        .collect::<Vec<_>>()
        .leak()
}

fn crate_names() -> &'static [FieldName] {
    KNOWN_NAMES
        .iter()
        .chain(CUSTOM_NAMES_MIXED_CASE)
        .map(|name| FieldName::try_from_bytes(name).expect("valid field name"))
        .collect::<Vec<_>>()
        .leak()
}

#[metabench::benchmark(PARSE_KNOWN_NAMES, RECOGNITION, "parse_known_names")]
#[bench::request(setup = known_names)]
fn parse_known_names(names: &'static [&'static [u8]]) -> usize {
    let mut recognized = 0;
    for name in names {
        let parsed = FieldName::try_from_bytes(black_box(name)).expect("valid field name");
        recognized += usize::from(parsed.index().is_some());
    }
    black_box(recognized)
}

#[metabench::benchmark(PARSE_CUSTOM_NAMES_LOWERCASE, RECOGNITION, "parse_custom_names_lowercase")]
#[bench::request(setup = custom_names_lowercase)]
fn parse_custom_names_lowercase(names: &'static [&'static [u8]]) -> usize {
    parse_custom_names(names)
}

#[metabench::benchmark(PARSE_CUSTOM_NAMES_MIXED_CASE, RECOGNITION, "parse_custom_names_mixed_case")]
#[bench::request(setup = custom_names_mixed_case)]
fn parse_custom_names_mixed_case(names: &'static [&'static [u8]]) -> usize {
    parse_custom_names(names)
}

fn parse_custom_names(names: &[&[u8]]) -> usize {
    let mut length = 0;
    for name in names {
        let parsed = FieldName::try_from_bytes(black_box(name)).expect("valid field name");
        length += parsed.as_str().len();
    }
    black_box(length)
}

#[metabench::benchmark(CUSTOM_NAME_INDEX, RECOGNITION, "custom_name_index")]
#[bench::request(setup = custom_header_names)]
fn custom_name_index(names: &'static [FieldName]) -> usize {
    let mut unknown = 0;
    for name in names {
        unknown += usize::from(black_box(name).index().is_none());
    }
    black_box(unknown)
}

#[metabench::benchmark(HTTP_NAMES_INTO_CRATE_NAMES, HTTP_CONVERSION, "http_names_into_crate_names")]
#[bench::request(setup = http_names)]
fn http_names_into_crate_names(names: &'static [http::HeaderName]) -> usize {
    let mut length = 0;
    for name in names {
        let converted = FieldName::from(black_box(name));
        length += converted.as_str().len();
    }
    black_box(length)
}

#[metabench::benchmark(CRATE_NAMES_INTO_HTTP_NAMES, HTTP_CONVERSION, "crate_names_into_http_names")]
#[bench::request(setup = crate_names)]
fn crate_names_into_http_names(names: &'static [FieldName]) -> usize {
    let mut length = 0;
    for name in names {
        let converted = http::HeaderName::from(black_box(name));
        length += converted.as_str().len();
    }
    black_box(length)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut recognition = criterion.benchmark_group(RECOGNITION);
    recognition.bench_function(BenchmarkId::new(PARSE_KNOWN_NAMES.benchmark_name(), "request"), |bencher| {
        bencher.iter_batched(known_names, parse_known_names, BatchSize::SmallInput);
    });
    recognition.bench_function(
        BenchmarkId::new(PARSE_CUSTOM_NAMES_LOWERCASE.benchmark_name(), "request"),
        |bencher| {
            bencher.iter_batched(custom_names_lowercase, parse_custom_names_lowercase, BatchSize::SmallInput);
        },
    );
    recognition.bench_function(
        BenchmarkId::new(PARSE_CUSTOM_NAMES_MIXED_CASE.benchmark_name(), "request"),
        |bencher| {
            bencher.iter_batched(custom_names_mixed_case, parse_custom_names_mixed_case, BatchSize::SmallInput);
        },
    );
    recognition.bench_function(BenchmarkId::new(CUSTOM_NAME_INDEX.benchmark_name(), "request"), |bencher| {
        bencher.iter_batched(custom_header_names, custom_name_index, BatchSize::SmallInput);
    });
    recognition.finish();

    let mut conversion = criterion.benchmark_group(HTTP_CONVERSION);
    conversion.bench_function(
        BenchmarkId::new(HTTP_NAMES_INTO_CRATE_NAMES.benchmark_name(), "request"),
        |bencher| {
            bencher.iter_batched(http_names, http_names_into_crate_names, BatchSize::SmallInput);
        },
    );
    conversion.bench_function(
        BenchmarkId::new(CRATE_NAMES_INTO_HTTP_NAMES.benchmark_name(), "request"),
        |bencher| {
            bencher.iter_batched(crate_names, crate_names_into_http_names, BatchSize::SmallInput);
        },
    );
    conversion.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        PARSE_KNOWN_NAMES,
        PARSE_CUSTOM_NAMES_LOWERCASE,
        PARSE_CUSTOM_NAMES_MIXED_CASE,
        CUSTOM_NAME_INDEX,
        HTTP_NAMES_INTO_CRATE_NAMES,
        CRATE_NAMES_INTO_HTTP_NAMES,
    ],
);
