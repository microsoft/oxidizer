// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Focused instruction-count and wall-clock cases for protocol header parsing.

use std::hint::black_box;
use std::sync::OnceLock;

use criterion::{BatchSize, BenchmarkId, Criterion};
use http::{HeaderMap, HeaderName, HeaderValue};
use http_headers::headers::{
    AcceptRanges, AccessControlAllowHeadersOwned, AccessControlAllowOriginOwned, ContentLength, ETagOwned, Host, IfNoneMatchOwned,
    RangeOwned, SecWebSocketExtensionsOwned, SecWebSocketProtocolOwned, SecWebSocketVersion,
};
use http_headers::{DecodeErrorKind, DecodeMode, FieldValue, SingleValueField};

const GROUP: &str = "http_headers_protocol/protocol";

fn map(name: &'static str, values: &'static [&'static str]) -> HeaderMap {
    let mut map = HeaderMap::with_capacity(values.len());
    let name = HeaderName::from_static(name);
    for value in values {
        map.append(&name, HeaderValue::from_static(value));
    }
    map
}

fn accept_ranges_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| map("accept-ranges", &["bytes", "items", "records"]))
}

fn content_length_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| map("content-length", &["18446744073709551615"]))
}

fn cors_many_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| {
        map(
            "access-control-allow-headers",
            &[
                "x-00", "x-01", "x-02", "x-03", "x-04", "x-05", "x-06", "x-07", "x-08", "x-09", "x-10", "x-11", "x-12", "x-13", "x-14",
                "x-15",
            ],
        )
    })
}

fn websocket_version_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| map("sec-websocket-version", &["13", "8", "\"7\""]))
}

fn host_ipv_future_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| map("host", &["[v1.fe80::a]:443"]))
}

fn host_domain() -> FieldValue {
    FieldValue::from_static("www.example.com:443")
}

fn host_idna() -> FieldValue {
    FieldValue::from_static("münich.example:443")
}

fn if_none_match() -> IfNoneMatchOwned {
    IfNoneMatchOwned::try_from(FieldValue::from_static(
        r#""short", W/"0123456789abcdefghijklmnopqrstuvwxyz", "obs\x80text""#,
    ))
    .expect("valid conditional tags")
}

fn byte_range() -> RangeOwned {
    RangeOwned::try_from("bytes=0-49, 100-, -500").expect("valid byte range")
}

fn extension_range() -> RangeOwned {
    RangeOwned::extension("items", "1-5").expect("valid extension range")
}

fn websocket_protocol() -> SecWebSocketProtocolOwned {
    SecWebSocketProtocolOwned::try_from("graphql-transport-ws, graphql-ws").expect("valid protocols")
}

fn websocket_extensions() -> SecWebSocketExtensionsOwned {
    SecWebSocketExtensionsOwned::try_from(r#"permessage-deflate; client_max_window_bits; mode="fast", x-test; token=value"#)
        .expect("valid extensions")
}

fn etag() -> ETagOwned {
    ETagOwned::weak("0123456789abcdefghijklmnopqrstuvwxyz").expect("valid entity tag")
}

fn sixteen_tags() -> impl Iterator<Item = ETagOwned> {
    (0..16).map(|index| ETagOwned::strong(format!("revision-{index}")).expect("valid entity tag"))
}

#[metabench::benchmark(ACCEPT_RANGES_OWNED_MULTILINE, GROUP, "accept_ranges_owned_multiline", gungraun_setup = accept_ranges_map)]
fn accept_ranges_owned_multiline(map: &'static HeaderMap) -> usize {
    AcceptRanges::owned(black_box(map))
        .expect("valid header")
        .expect("present header")
        .units()
        .count()
}

#[metabench::benchmark(CONTENT_LENGTH_20_DIGITS, GROUP, "content_length_20_digits", gungraun_setup = content_length_map)]
fn content_length_20_digits(map: &'static HeaderMap) -> u64 {
    ContentLength::view(black_box(map))
        .expect("valid header")
        .expect("present header")
        .get()
}

#[metabench::benchmark(CONDITIONAL_TAGS_LONG, GROUP, "conditional_tags_long", gungraun_setup = if_none_match)]
fn conditional_tags_long(value: IfNoneMatchOwned) -> (usize, IfNoneMatchOwned) {
    let length = value.tags().map(|tag| tag.as_bytes().len()).sum();
    (black_box(length), value)
}

#[metabench::benchmark(RANGE_BYTE_MEMBERS, GROUP, "range_byte_members", gungraun_setup = byte_range)]
fn range_byte_members(value: RangeOwned) -> (usize, RangeOwned) {
    let count = value.byte_ranges().expect("byte unit").count();
    (black_box(count), value)
}

#[metabench::benchmark(RANGE_EXTENSION_PROJECTION, GROUP, "range_extension_projection", gungraun_setup = extension_range)]
fn range_extension_projection(value: RangeOwned) -> (usize, RangeOwned) {
    let length = value.extension_range_set().expect("extension unit").len();
    (black_box(length), value)
}

#[metabench::benchmark(CORS_SINGLETON_CONVERSION, GROUP, "cors_singleton_conversion")]
fn cors_singleton_conversion() -> AccessControlAllowHeadersOwned {
    AccessControlAllowHeadersOwned::try_from(black_box(FieldValue::from_static("x-custom-header"))).expect("valid CORS list")
}

#[metabench::benchmark(CORS_OWNED_16_LINES, GROUP, "cors_owned_16_lines", gungraun_setup = cors_many_map)]
fn cors_owned_16_lines(map: &'static HeaderMap) -> usize {
    AccessControlAllowHeadersOwned::from_field_values(map.get_all("access-control-allow-headers").iter().map(FieldValue::from).collect())
        .expect("valid CORS list")
        .len()
}

#[metabench::benchmark(CONDITIONAL_OWNED_16_TAGS, GROUP, "conditional_owned_16_tags")]
fn conditional_owned_16_tags() -> usize {
    IfNoneMatchOwned::from_tags(sixteen_tags()).expect("valid tags").tags().count()
}

#[metabench::benchmark(CORS_IPV6_ORIGIN, GROUP, "cors_ipv6_origin")]
fn cors_ipv6_origin() -> AccessControlAllowOriginOwned {
    AccessControlAllowOriginOwned::try_from(black_box(FieldValue::from_static("https://[2001:db8::1]:8443")))
        .expect("canonical IPv6 origin")
}

#[metabench::benchmark(WEBSOCKET_PROTOCOL_READ, GROUP, "websocket_protocol_read", gungraun_setup = websocket_protocol)]
fn websocket_protocol_read(value: SecWebSocketProtocolOwned) -> (usize, SecWebSocketProtocolOwned) {
    let length = value.protocols().map(|item| item.expect("validated protocol").len()).sum();
    (black_box(length), value)
}

#[metabench::benchmark(WEBSOCKET_EXTENSION_READ, GROUP, "websocket_extension_read", gungraun_setup = websocket_extensions)]
fn websocket_extension_read(value: SecWebSocketExtensionsOwned) -> (usize, SecWebSocketExtensionsOwned) {
    let count = value
        .extensions()
        .map(|extension| extension.expect("validated extension").parameters().count())
        .sum();
    (black_box(count), value)
}

#[metabench::benchmark(WEBSOCKET_VERSION_LATE_QUOTE, GROUP, "websocket_version_late_quote", gungraun_setup = websocket_version_map)]
fn websocket_version_late_quote(map: &'static HeaderMap) -> DecodeErrorKind {
    SecWebSocketVersion::view(black_box(map))
        .expect_err("quoted version is invalid")
        .kind()
}

#[metabench::benchmark(HOST_IPV_FUTURE, GROUP, "host_ipv_future", gungraun_setup = host_ipv_future_map)]
fn host_ipv_future(map: &'static HeaderMap) -> usize {
    Host::view(black_box(map)).expect("valid host").expect("present host").host().len()
}

#[metabench::benchmark(HOST_RELAXED_DOMAIN, GROUP, "host_relaxed_domain", gungraun_setup = host_domain)]
fn host_relaxed_domain(value: FieldValue) -> (usize, FieldValue) {
    let view = <Host as SingleValueField>::decode_view_with(value.as_field_value_ref(), DecodeMode::Relaxed).expect("valid relaxed host");
    (black_box(view.host().len() + view.port().map_or(0, str::len)), value)
}

#[metabench::benchmark(HOST_RELAXED_IDNA, GROUP, "host_relaxed_idna", gungraun_setup = host_idna)]
fn host_relaxed_idna(value: FieldValue) -> (usize, FieldValue) {
    let view = <Host as SingleValueField>::decode_view_with(value.as_field_value_ref(), DecodeMode::Relaxed).expect("valid relaxed host");
    (black_box(view.host().len() + view.port().map_or(0, str::len)), value)
}

#[metabench::benchmark(ETAG_OPAQUE_READ, GROUP, "etag_opaque_read", gungraun_setup = etag)]
fn etag_opaque_read(value: ETagOwned) -> (usize, ETagOwned) {
    let length = value.opaque_tag().expect("valid stored tag").len();
    (black_box(length), value)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(GROUP);
    macro_rules! borrowed {
        ($id:ident, $setup:ident, $function:ident) => {
            group.bench_function(BenchmarkId::new($id.benchmark_name(), stringify!($function)), |bencher| {
                bencher.iter_batched($setup, $function, BatchSize::SmallInput);
            });
        };
    }
    borrowed!(ACCEPT_RANGES_OWNED_MULTILINE, accept_ranges_map, accept_ranges_owned_multiline);
    borrowed!(CONTENT_LENGTH_20_DIGITS, content_length_map, content_length_20_digits);
    borrowed!(CONDITIONAL_TAGS_LONG, if_none_match, conditional_tags_long);
    borrowed!(RANGE_BYTE_MEMBERS, byte_range, range_byte_members);
    borrowed!(RANGE_EXTENSION_PROJECTION, extension_range, range_extension_projection);
    group.bench_function(CORS_SINGLETON_CONVERSION.benchmark_name(), |bencher| {
        bencher.iter(cors_singleton_conversion);
    });
    borrowed!(CORS_OWNED_16_LINES, cors_many_map, cors_owned_16_lines);
    group.bench_function(CONDITIONAL_OWNED_16_TAGS.benchmark_name(), |bencher| {
        bencher.iter(conditional_owned_16_tags);
    });
    group.bench_function(CORS_IPV6_ORIGIN.benchmark_name(), |bencher| bencher.iter(cors_ipv6_origin));
    borrowed!(WEBSOCKET_PROTOCOL_READ, websocket_protocol, websocket_protocol_read);
    borrowed!(WEBSOCKET_EXTENSION_READ, websocket_extensions, websocket_extension_read);
    borrowed!(WEBSOCKET_VERSION_LATE_QUOTE, websocket_version_map, websocket_version_late_quote);
    borrowed!(HOST_IPV_FUTURE, host_ipv_future_map, host_ipv_future);
    borrowed!(HOST_RELAXED_DOMAIN, host_domain, host_relaxed_domain);
    borrowed!(HOST_RELAXED_IDNA, host_idna, host_relaxed_idna);
    borrowed!(ETAG_OPAQUE_READ, etag, etag_opaque_read);
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        ACCEPT_RANGES_OWNED_MULTILINE,
        CONTENT_LENGTH_20_DIGITS,
        CONDITIONAL_TAGS_LONG,
        RANGE_BYTE_MEMBERS,
        RANGE_EXTENSION_PROJECTION,
        CORS_SINGLETON_CONVERSION,
        CORS_OWNED_16_LINES,
        CONDITIONAL_OWNED_16_TAGS,
        CORS_IPV6_ORIGIN,
        WEBSOCKET_PROTOCOL_READ,
        WEBSOCKET_EXTENSION_READ,
        WEBSOCKET_VERSION_LATE_QUOTE,
        HOST_IPV_FUTURE,
        HOST_RELAXED_DOMAIN,
        HOST_RELAXED_IDNA,
        ETAG_OPAQUE_READ,
    ]
);
