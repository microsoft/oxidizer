// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Location decode, retained reads, and redirect forwarding workloads.
//!
//! The caller baselines reproduce pre-metadata strict validation followed by
//! one caller-side RFC 3986 parse, retained across 1/2/8 reads. They do not
//! charge a fresh parse for every accessor.

use std::hint::black_box;
use std::sync::OnceLock;

use criterion::{BatchSize, Criterion};
use fluent_uri::Uri;
use http::{HeaderMap, HeaderValue};
use http_headers::headers::{Location, LocationOwned, LocationView, UriReference};
use http_headers::{DecodeError, DecodeErrorKind, DecodeMode, FieldName, FieldValue, SingleValueField};

const GROUP: &str = "http_headers_location_semantics/reads";
const SIMPLE: &str = "https://example.com:8443/docs/next?tab=1#top";
const GENERAL: &str = "https://user:secret@[2001:db8::1]:8443/a%2Fb?tab=%31#top";
const RELAXED: &str = r"https:\\example.com:8443\docs\next?tab=1#top";

fn simple() -> FieldValue {
    FieldValue::from_static(SIMPLE)
}

fn general() -> FieldValue {
    FieldValue::from_static(GENERAL)
}

fn relaxed() -> FieldValue {
    FieldValue::from_static(RELAXED)
}

fn retained() -> &'static LocationView<'static> {
    static FIELD: FieldValue = FieldValue::from_static(SIMPLE);
    static VIEW: OnceLock<LocationView<'static>> = OnceLock::new();
    VIEW.get_or_init(|| <Location as SingleValueField>::decode_view(FIELD.as_field_value_ref()).expect("valid fixture"))
}

fn retained_relaxed() -> &'static LocationView<'static> {
    static FIELD: FieldValue = FieldValue::from_static(RELAXED);
    static VIEW: OnceLock<LocationView<'static>> = OnceLock::new();
    VIEW.get_or_init(|| {
        <Location as SingleValueField>::decode_view_with(FIELD.as_field_value_ref(), DecodeMode::Relaxed).expect("valid fixture")
    })
}

fn owned() -> LocationOwned {
    LocationOwned::try_from(SIMPLE).expect("valid fixture")
}

fn redirect_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("location", HeaderValue::from_static(SIMPLE));
    headers
}

fn old_decode(value: &FieldValue) -> Result<&str, DecodeError> {
    let invalid = || DecodeError::new(&FieldName::Location, DecodeErrorKind::InvalidSyntax);
    if let Some(text) = http_headers_simd::as_simple_uri_reference(value.as_bytes()) {
        return Ok(text);
    }
    let text = std::str::from_utf8(value.as_bytes()).map_err(|_invalid| invalid())?;
    i32::try_from(text.len()).map_err(|_invalid| invalid())?;
    Uri::parse(text).map_err(|_invalid| invalid())?;
    Ok(text)
}

fn observe(uri: UriReference<'_>) -> usize {
    uri.scheme().map_or(0, str::len)
        + uri.authority().map_or(0, |authority| {
            authority.userinfo().map_or(0, str::len) + authority.host().len() + authority.port().map_or(0, str::len)
        })
        + uri.path().len()
        + uri.query().map_or(0, str::len)
        + uri.fragment().map_or(0, str::len)
}

fn observe_caller(uri: &Uri<&str>) -> usize {
    uri.scheme().map_or(0, |scheme| scheme.as_str().len())
        + uri.authority().map_or(0, |authority| {
            authority.userinfo().map_or(0, |userinfo| userinfo.as_str().len())
                + authority.host().as_str().len()
                + authority.port().map_or(0, str::len)
        })
        + uri.path().as_str().len()
        + uri.query().map_or(0, |query| query.as_str().len())
        + uri.fragment().map_or(0, |fragment| fragment.as_str().len())
}

fn structured_reads<const READS: usize>(value: FieldValue) -> (usize, FieldValue) {
    let view = <Location as SingleValueField>::decode_view(black_box(value.as_field_value_ref())).expect("valid fixture");
    let uri = view.uri_reference();
    let sum = (0..READS).map(|_| black_box(observe(black_box(uri)))).sum();
    (sum, value)
}

fn caller_reads<const READS: usize>(value: FieldValue) -> (usize, FieldValue) {
    let text = old_decode(black_box(&value)).expect("valid fixture");
    let uri = Uri::parse(black_box(text)).expect("validated reference");
    let sum = (0..READS).map(|_| black_box(observe_caller(black_box(&uri)))).sum();
    (sum, value)
}

#[metabench::benchmark(DECODE_SIMPLE, GROUP, "decode_simple", gungraun_setup = simple)]
fn decode_simple(value: FieldValue) -> (usize, FieldValue) {
    let view = <Location as SingleValueField>::decode_view(black_box(value.as_field_value_ref())).expect("valid fixture");
    let length = black_box(view).as_bytes().len();
    (length, value)
}

#[metabench::benchmark(DECODE_GENERAL, GROUP, "decode_general", gungraun_setup = general)]
fn decode_general(value: FieldValue) -> (usize, FieldValue) {
    decode_simple(value)
}

#[metabench::benchmark(DECODE_RELAXED, GROUP, "decode_relaxed", gungraun_setup = relaxed)]
fn decode_relaxed(value: FieldValue) -> (usize, FieldValue) {
    let view = <Location as SingleValueField>::decode_view_with(black_box(value.as_field_value_ref()), DecodeMode::Relaxed)
        .expect("valid fixture");
    let length = black_box(view).as_bytes().len();
    (length, value)
}

#[metabench::benchmark(STRUCTURED_1, GROUP, "structured_1", gungraun_setup = simple)]
fn structured_1(value: FieldValue) -> (usize, FieldValue) {
    structured_reads::<1>(value)
}

#[metabench::benchmark(STRUCTURED_2, GROUP, "structured_2", gungraun_setup = simple)]
fn structured_2(value: FieldValue) -> (usize, FieldValue) {
    structured_reads::<2>(value)
}

#[metabench::benchmark(STRUCTURED_8, GROUP, "structured_8", gungraun_setup = simple)]
fn structured_8(value: FieldValue) -> (usize, FieldValue) {
    structured_reads::<8>(value)
}

#[metabench::benchmark(CALLER_1, GROUP, "caller_parse_1", gungraun_setup = simple)]
fn caller_1(value: FieldValue) -> (usize, FieldValue) {
    caller_reads::<1>(value)
}

#[metabench::benchmark(CALLER_2, GROUP, "caller_parse_2", gungraun_setup = simple)]
fn caller_2(value: FieldValue) -> (usize, FieldValue) {
    caller_reads::<2>(value)
}

#[metabench::benchmark(CALLER_8, GROUP, "caller_parse_8", gungraun_setup = simple)]
fn caller_8(value: FieldValue) -> (usize, FieldValue) {
    caller_reads::<8>(value)
}

#[metabench::benchmark(STRUCTURED_GENERAL_8, GROUP, "structured_general_8", gungraun_setup = general)]
fn structured_general_8(value: FieldValue) -> (usize, FieldValue) {
    structured_reads::<8>(value)
}

#[metabench::benchmark(CALLER_GENERAL_8, GROUP, "caller_general_8", gungraun_setup = general)]
fn caller_general_8(value: FieldValue) -> (usize, FieldValue) {
    caller_reads::<8>(value)
}

#[metabench::benchmark(STRUCTURED_RELAXED_8, GROUP, "structured_relaxed_8", gungraun_setup = relaxed)]
fn structured_relaxed_8(value: FieldValue) -> (usize, FieldValue) {
    let view = <Location as SingleValueField>::decode_view_with(black_box(value.as_field_value_ref()), DecodeMode::Relaxed)
        .expect("valid fixture");
    let uri = view.uri_reference();
    let sum = (0..8).map(|_| black_box(observe(black_box(uri)))).sum();
    (sum, value)
}

#[metabench::benchmark(RETAINED_1, GROUP, "retained_1", gungraun_setup = retained)]
fn retained_1(value: &'static LocationView<'static>) -> usize {
    observe(black_box(value.uri_reference()))
}

#[metabench::benchmark(RETAINED_2, GROUP, "retained_2", gungraun_setup = retained)]
fn retained_2(value: &'static LocationView<'static>) -> usize {
    let uri = value.uri_reference();
    (0..2).map(|_| black_box(observe(black_box(uri)))).sum()
}

#[metabench::benchmark(RETAINED_8, GROUP, "retained_8", gungraun_setup = retained)]
fn retained_8(value: &'static LocationView<'static>) -> usize {
    let uri = value.uri_reference();
    (0..8).map(|_| black_box(observe(black_box(uri)))).sum()
}

#[metabench::benchmark(RETAINED_RELAXED_8, GROUP, "retained_relaxed_8", gungraun_setup = retained_relaxed)]
fn retained_relaxed_8(value: &'static LocationView<'static>) -> usize {
    retained_8(value)
}

#[metabench::benchmark(RETAINED_OWNED_8, GROUP, "retained_owned_8", gungraun_setup = owned)]
fn retained_owned_8(value: LocationOwned) -> (usize, LocationOwned) {
    let uri = value.uri_reference();
    let sum = (0..8).map(|_| black_box(observe(black_box(uri)))).sum();
    (sum, value)
}

#[metabench::benchmark(INSPECT_FORWARD, GROUP, "inspect_forward", gungraun_setup = redirect_headers)]
fn inspect_forward(headers: HeaderMap) -> (usize, HeaderMap, HeaderMap) {
    let view = Location::view(black_box(&headers))
        .expect("valid fixture")
        .expect("present fixture");
    let uri = view.uri_reference();
    let observed = observe(black_box(uri));
    let mut forwarded = HeaderMap::new();
    if black_box(uri.path().starts_with("/docs") && uri.query().is_some()) {
        view.insert_into(&mut forwarded).expect("valid forwarding");
    }
    (observed, headers, forwarded)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    // Only call uninstrumented helpers here: metabench measures the first
    // instrumented invocation in an allocation worker.
    assert_eq!(caller_reads::<1>(simple()).0, structured_reads::<1>(simple()).0);
    assert_eq!(caller_reads::<2>(simple()).0, structured_reads::<2>(simple()).0);
    assert_eq!(caller_reads::<8>(simple()).0, structured_reads::<8>(simple()).0);
    assert_eq!(caller_reads::<8>(general()).0, structured_reads::<8>(general()).0);
    assert_eq!(observe(retained_relaxed().uri_reference()), observe(retained().uri_reference()));

    let mut group = criterion.benchmark_group(DECODE_SIMPLE.group_name());
    macro_rules! case {
        ($id:ident, $setup:ident, $function:ident) => {
            group.bench_function($id.benchmark_name(), |bencher| {
                bencher.iter_batched($setup, $function, BatchSize::SmallInput);
            });
        };
    }
    case!(DECODE_SIMPLE, simple, decode_simple);
    case!(DECODE_GENERAL, general, decode_general);
    case!(DECODE_RELAXED, relaxed, decode_relaxed);
    case!(STRUCTURED_1, simple, structured_1);
    case!(STRUCTURED_2, simple, structured_2);
    case!(STRUCTURED_8, simple, structured_8);
    case!(CALLER_1, simple, caller_1);
    case!(CALLER_2, simple, caller_2);
    case!(CALLER_8, simple, caller_8);
    case!(STRUCTURED_GENERAL_8, general, structured_general_8);
    case!(CALLER_GENERAL_8, general, caller_general_8);
    case!(STRUCTURED_RELAXED_8, relaxed, structured_relaxed_8);
    case!(RETAINED_1, retained, retained_1);
    case!(RETAINED_2, retained, retained_2);
    case!(RETAINED_8, retained, retained_8);
    case!(RETAINED_RELAXED_8, retained_relaxed, retained_relaxed_8);
    case!(RETAINED_OWNED_8, owned, retained_owned_8);
    case!(INSPECT_FORWARD, redirect_headers, inspect_forward);
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        DECODE_SIMPLE,
        DECODE_GENERAL,
        DECODE_RELAXED,
        STRUCTURED_1,
        STRUCTURED_2,
        STRUCTURED_8,
        CALLER_1,
        CALLER_2,
        CALLER_8,
        STRUCTURED_GENERAL_8,
        CALLER_GENERAL_8,
        STRUCTURED_RELAXED_8,
        RETAINED_1,
        RETAINED_2,
        RETAINED_8,
        RETAINED_RELAXED_8,
        RETAINED_OWNED_8,
        INSPECT_FORWARD,
    ]
);
