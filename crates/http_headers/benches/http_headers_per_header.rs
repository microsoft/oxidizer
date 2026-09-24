// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unified per-header comparisons against `headers` 0.4.1.

use std::hint::black_box;
use std::sync::OnceLock;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion};
use headers::HeaderMapExt as TheirMapExt;
use http::{HeaderMap, HeaderName, HeaderValue};
use http_headers::Field;
use http_headers::headers::{
    Accept, AcceptEncoding, AcceptLanguage, AccessControlAllowCredentials, AccessControlAllowHeaders, AccessControlAllowOrigin,
    AccessControlExposeHeaders, AccessControlMaxAge, AccessControlRequestHeaders, AccessControlRequestMethod, Allow, Authorization, Basic,
    Bearer, CacheControl, ContentLength, ContentRange, ContentSecurityPolicy, ContentType, ETag, Host, IfMatch, IfModifiedSince,
    IfNoneMatch, IfRange, IfUnmodifiedSince, LastModified, Location, ReferrerPolicy, SecWebSocketAccept, SecWebSocketExtensions,
    SecWebSocketKey, SecWebSocketProtocol, SecWebSocketVersion, Server, SetCookie, StrictTransportSecurity, UserAgent, XContentTypeOptions,
};
use paste::paste;

#[path = "http_headers_common_values.rs"]
mod common_values;
#[expect(
    dead_code,
    reason = "the shared operations module also supports benchmark targets with different inventories"
)]
#[path = "http_headers_operations.rs"]
mod operations;

fn map(name: &'static str, values: &'static [&'static str]) -> HeaderMap {
    let mut map = HeaderMap::with_capacity(1);
    let name = HeaderName::from_static(name);
    for value in values {
        let mut value = HeaderValue::from_bytes(value.as_bytes()).expect("valid benchmark fixture");
        value.set_sensitive(matches!(name.as_str(), "authorization" | "set-cookie"));
        map.append(&name, value);
    }
    map
}

fn consume<T>(value: T) {
    drop(black_box(value));
}

fn tuned_criterion() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .sample_size(60)
}

macro_rules! compare_header {
    ($id:ident, $name:literal, $values:expr, $ours:ty, $theirs:ty) => {
        paste! {
            fn [<$id _map>]() -> &'static HeaderMap {
                static MAP: OnceLock<HeaderMap> = OnceLock::new();
                MAP.get_or_init(|| map($name, $values))
            }

            fn [<$id _headers>](map: &'static HeaderMap) {
                operations::headers_owned::<$theirs>(map);
            }

            fn [<$id _owned>](map: &'static HeaderMap) {
                operations::http_headers_owned::<$ours>(map);
            }

            fn [<$id _borrowed>](map: &'static HeaderMap) {
                operations::http_headers_borrowed::<$ours>(map);
            }
        }
    };
}

macro_rules! compare_tag_header {
    (
        $id:ident,
        $name:literal,
        $values:expr,
        $ours:ty,
        $theirs:ty,
        ours = |$header:ident| $ours_read:expr
    ) => {
        paste! {
            fn [<$id _map>]() -> &'static HeaderMap {
                static MAP: OnceLock<HeaderMap> = OnceLock::new();
                MAP.get_or_init(|| map($name, $values))
            }

            fn [<$id _headers>](map: &'static HeaderMap) -> bool {
                static COMPARISON: OnceLock<headers::ETag> = OnceLock::new();
                let comparison = COMPARISON.get_or_init(|| {
                    "\"benchmark-never-matches\"".parse().expect("valid comparison ETag")
                });
                let header = TheirMapExt::typed_try_get::<$theirs>(black_box(map))
                    .expect("fixture must decode")
                    .expect("fixture must be present");
                let result = header.precondition_passes(black_box(comparison));
                consume(header);
                black_box(result)
            }

            fn [<$id _owned>](map: &'static HeaderMap) -> bool {
                let $header = <$ours as Field>::owned(black_box(map))
                    .expect("fixture must decode")
                    .expect("fixture must be present");
                let result = $ours_read;
                consume($header);
                black_box(result)
            }

            fn [<$id _borrowed>](map: &'static HeaderMap) -> bool {
                let $header = <$ours as Field>::view(black_box(map))
                    .expect("fixture must decode")
                    .expect("fixture must be present");
                let result = $ours_read;
                consume($header);
                black_box(result)
            }
        }
    };
}

macro_rules! compare_iter_header {
    (
        $id:ident,
        $name:literal,
        $values:expr,
        $ours:ty,
        $theirs:ty,
        ours = $our_iter:ident,
        theirs = $their_iter:ident
    ) => {
        paste! {
            fn [<$id _map>]() -> &'static HeaderMap {
                static MAP: OnceLock<HeaderMap> = OnceLock::new();
                MAP.get_or_init(|| map($name, $values))
            }

            fn [<$id _headers>](map: &'static HeaderMap) -> usize {
                let header = TheirMapExt::typed_try_get::<$theirs>(black_box(map))
                    .expect("fixture must decode")
                    .expect("fixture must be present");
                let count = header.$their_iter().map(black_box).count();
                consume(header);
                black_box(count)
            }

            fn [<$id _owned>](map: &'static HeaderMap) -> usize {
                let header = <$ours as Field>::owned(black_box(map))
                    .expect("fixture must decode")
                    .expect("fixture must be present");
                let count = header.$our_iter().map(black_box).count();
                consume(header);
                black_box(count)
            }

            fn [<$id _borrowed>](map: &'static HeaderMap) -> usize {
                let header = <$ours as Field>::view(black_box(map))
                    .expect("fixture must decode")
                    .expect("fixture must be present");
                let count = header.$our_iter().map(black_box).count();
                consume(header);
                black_box(count)
            }
        }
    };
}

macro_rules! compare_shared_iter_header {
    (
        $id:ident,
        $name:literal,
        $values:expr,
        headers = $headers:path,
        owned = $owned:path,
        borrowed = $borrowed:path
    ) => {
        paste! {
            fn [<$id _map>]() -> &'static HeaderMap {
                static MAP: OnceLock<HeaderMap> = OnceLock::new();
                MAP.get_or_init(|| map($name, $values))
            }

            fn [<$id _headers>](map: &'static HeaderMap) -> usize {
                $headers(map)
            }

            fn [<$id _owned>](map: &'static HeaderMap) -> usize {
                $owned(map)
            }

            fn [<$id _borrowed>](map: &'static HeaderMap) -> usize {
                $borrowed(map)
            }
        }
    };
}

macro_rules! measure_header {
    ($id:ident, $name:literal, $values:expr, $ours:ty) => {
        paste! {
            fn [<$id _map>]() -> &'static HeaderMap {
                static MAP: OnceLock<HeaderMap> = OnceLock::new();
                MAP.get_or_init(|| map($name, $values))
            }

            fn [<$id _owned>](map: &'static HeaderMap) {
                operations::http_headers_owned::<$ours>(map);
            }

            fn [<$id _borrowed>](map: &'static HeaderMap) {
                operations::http_headers_borrowed::<$ours>(map);
            }
        }
    };
}

measure_header!(
    accept,
    "accept",
    &[
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"
    ],
    Accept
);
measure_header!(accept_encoding, "accept-encoding", &["gzip, deflate, br, zstd"], AcceptEncoding);
measure_header!(
    accept_language,
    "accept-language",
    &["en-US,en;q=0.9,fr-FR;q=0.8,fr;q=0.7"],
    AcceptLanguage
);
compare_shared_iter_header!(
    accept_ranges,
    "accept-ranges",
    &["bytes"],
    headers = operations::headers_accept_ranges,
    owned = operations::http_headers_accept_ranges_owned,
    borrowed = operations::http_headers_accept_ranges_borrowed
);
compare_header!(
    access_control_allow_credentials,
    "access-control-allow-credentials",
    &["true"],
    AccessControlAllowCredentials,
    headers::AccessControlAllowCredentials
);
compare_iter_header!(
    access_control_allow_headers,
    "access-control-allow-headers",
    &["content-type, x-request-id"],
    AccessControlAllowHeaders,
    headers::AccessControlAllowHeaders,
    ours = iter,
    theirs = iter
);
compare_shared_iter_header!(
    access_control_allow_methods,
    "access-control-allow-methods",
    &["GET, POST"],
    headers = operations::headers_allow_methods,
    owned = operations::http_headers_allow_methods_owned,
    borrowed = operations::http_headers_allow_methods_borrowed
);
compare_header!(
    access_control_allow_origin,
    "access-control-allow-origin",
    &["https://example.com"],
    AccessControlAllowOrigin,
    headers::AccessControlAllowOrigin
);
compare_iter_header!(
    access_control_expose_headers,
    "access-control-expose-headers",
    &["etag, x-request-id"],
    AccessControlExposeHeaders,
    headers::AccessControlExposeHeaders,
    ours = iter,
    theirs = iter
);
compare_header!(
    access_control_max_age,
    "access-control-max-age",
    &["600"],
    AccessControlMaxAge,
    headers::AccessControlMaxAge
);
compare_iter_header!(
    access_control_request_headers,
    "access-control-request-headers",
    &["content-type, x-request-id"],
    AccessControlRequestHeaders,
    headers::AccessControlRequestHeaders,
    ours = iter,
    theirs = iter
);
compare_header!(
    access_control_request_method,
    "access-control-request-method",
    &["POST"],
    AccessControlRequestMethod,
    headers::AccessControlRequestMethod
);
compare_iter_header!(allow, "allow", &["GET, POST"], Allow, headers::Allow, ours = items, theirs = iter);
compare_header!(
    authorization_basic,
    "authorization",
    &[common_values::BASIC_AUTHORIZATION],
    Authorization<Basic>,
    headers::Authorization<headers::authorization::Basic>
);
compare_header!(
    authorization_bearer,
    "authorization",
    &[
        "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyLCJleHAiOjE1MTYyNDI2MjIsImF1ZCI6Imh0dHBzOi8vYXBpLmV4YW1wbGUuY29tIiwiaXNzIjoiaHR0cHM6Ly9hdXRoLmV4YW1wbGUuY29tIiwic2NvcGUiOiJyZWFkOnByb2ZpbGUgd3JpdGU6cHJvZmlsZSJ9.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXkw"
    ],
    Authorization<Bearer>,
    headers::Authorization<headers::authorization::Bearer>
);
compare_header!(
    cache_control,
    "cache-control",
    &["max-age=3600, private"],
    CacheControl,
    headers::CacheControl
);
compare_header!(content_length, "content-length", &["348"], ContentLength, headers::ContentLength);
compare_header!(
    content_range,
    "content-range",
    &["bytes 0-499/1234"],
    ContentRange,
    headers::ContentRange
);
measure_header!(
    content_security_policy,
    "content-security-policy",
    &[
        "default-src 'self'; script-src 'self' 'unsafe-inline' https://cdn.example.com https://analytics.example.com; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; img-src 'self' data: https:; font-src 'self' https://fonts.gstatic.com; connect-src 'self' https://api.example.com; frame-ancestors 'none'; base-uri 'self'; form-action 'self'"
    ],
    ContentSecurityPolicy
);
compare_header!(
    content_type,
    "content-type",
    &["application/json; charset=utf-8"],
    ContentType,
    headers::ContentType
);
compare_header!(etag, "etag", &["\"revision-42\""], ETag, headers::ETag);
compare_header!(host, "host", &["example.com:8443"], Host, headers::Host);
compare_tag_header!(
    if_match,
    "if-match",
    &["\"a\", W/\"b\""],
    IfMatch,
    headers::IfMatch,
    ours = |header| header.is_wildcard()
        || header
            .tags()
            .any(|tag| { !tag.is_weak() && tag.opaque_tag() == black_box(b"benchmark-never-matches") })
);
compare_header!(
    if_modified_since,
    "if-modified-since",
    &["Sun, 06 Nov 1994 08:49:37 GMT"],
    IfModifiedSince,
    headers::IfModifiedSince
);
compare_tag_header!(
    if_none_match,
    "if-none-match",
    &["W/\"a\", \"b\""],
    IfNoneMatch,
    headers::IfNoneMatch,
    ours = |header| !(header.is_wildcard() || header.tags().any(|tag| tag.opaque_tag() == black_box(b"benchmark-never-matches")))
);
compare_header!(if_range, "if-range", &["\"revision-42\""], IfRange, headers::IfRange);
compare_header!(
    if_unmodified_since,
    "if-unmodified-since",
    &["Sun, 06 Nov 1994 08:49:37 GMT"],
    IfUnmodifiedSince,
    headers::IfUnmodifiedSince
);
compare_header!(
    last_modified,
    "last-modified",
    &["Sun, 06 Nov 1994 08:49:37 GMT"],
    LastModified,
    headers::LastModified
);
compare_header!(
    location,
    "location",
    &["https://example.com/en-us/docs/reference/index.html?utm_source=newsletter&utm_campaign=spring&page=3"],
    Location,
    headers::Location
);
compare_header!(
    range,
    "range",
    &["bytes=0-499, 1000-"],
    http_headers::headers::Range,
    headers::Range
);
compare_header!(
    referrer_policy,
    "referrer-policy",
    &["strict-origin-when-cross-origin"],
    ReferrerPolicy,
    headers::ReferrerPolicy
);
compare_header!(
    sec_websocket_accept,
    "sec-websocket-accept",
    &["s3pPLMBiTxaQ9kYGzzhZRbK+xOo="],
    SecWebSocketAccept,
    headers::SecWebsocketAccept
);
measure_header!(
    sec_websocket_extensions,
    "sec-websocket-extensions",
    &["permessage-deflate; client_max_window_bits"],
    SecWebSocketExtensions
);
compare_header!(
    sec_websocket_key,
    "sec-websocket-key",
    &["dGhlIHNhbXBsZSBub25jZQ=="],
    SecWebSocketKey,
    headers::SecWebsocketKey
);
measure_header!(
    sec_websocket_protocol,
    "sec-websocket-protocol",
    &["chat, superchat"],
    SecWebSocketProtocol
);
compare_header!(
    sec_websocket_version,
    "sec-websocket-version",
    &["13"],
    SecWebSocketVersion,
    headers::SecWebsocketVersion
);
measure_header!(
    sec_websocket_version_advertisement,
    "sec-websocket-version",
    &["7, 8, 13"],
    SecWebSocketVersion
);
compare_header!(server, "server", &["example/1.0"], Server, headers::Server);
compare_header!(
    set_cookie,
    "set-cookie",
    &["session=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0; Path=/; Domain=example.com; Max-Age=3600; Secure; HttpOnly; SameSite=Lax"],
    SetCookie,
    headers::SetCookie
);
compare_header!(
    strict_transport_security,
    "strict-transport-security",
    &["max-age=31536000; includeSubDomains"],
    StrictTransportSecurity,
    headers::StrictTransportSecurity
);
compare_header!(
    user_agent,
    "user-agent",
    &["Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"],
    UserAgent,
    headers::UserAgent
);
compare_shared_iter_header!(
    vary,
    "vary",
    &["accept-encoding, origin"],
    headers = operations::headers_vary,
    owned = operations::http_headers_vary_owned,
    borrowed = operations::http_headers_vary_borrowed
);
measure_header!(x_content_type_options, "x-content-type-options", &["nosniff"], XContentTypeOptions);

macro_rules! finish {
    (
        supported = [$($supported:ident),+ $(,)?],
        ours = [$($ours:ident),+ $(,)?],
    ) => {
        paste! {
            #[derive(Clone, Copy)]
            struct BenchmarkCase {
                map: &'static HeaderMap,
                operation: fn(&'static HeaderMap),
            }

            impl BenchmarkCase {
                fn run(self) {
                    (self.operation)(self.map);
                }
            }

            $(
                fn [<$supported _headers_case>]() -> BenchmarkCase {
                    BenchmarkCase {
                        map: [<$supported _map>](),
                        operation: |map| consume([<$supported _headers>](map)),
                    }
                }

                fn [<$supported _owned_case>]() -> BenchmarkCase {
                    BenchmarkCase {
                        map: [<$supported _map>](),
                        operation: |map| consume([<$supported _owned>](map)),
                    }
                }

                fn [<$supported _borrowed_case>]() -> BenchmarkCase {
                    BenchmarkCase {
                        map: [<$supported _map>](),
                        operation: |map| consume([<$supported _borrowed>](map)),
                    }
                }
            )+

            $(
                fn [<$ours _owned_case>]() -> BenchmarkCase {
                    BenchmarkCase {
                        map: [<$ours _map>](),
                        operation: |map| consume([<$ours _owned>](map)),
                    }
                }

                fn [<$ours _borrowed_case>]() -> BenchmarkCase {
                    BenchmarkCase {
                        map: [<$ours _map>](),
                        operation: |map| consume([<$ours _borrowed>](map)),
                    }
                }
            )+

            #[metabench::benchmark(COMPETITOR, "http_headers_per_header/per_header", "headers")]
            $(#[bench::$supported(setup = [<$supported _headers_case>])])+
            fn competitor(case: BenchmarkCase) {
                case.run();
            }

            #[metabench::benchmark(OWNED, "http_headers_per_header/per_header", "http_headers_owned")]
            $(#[bench::$supported(setup = [<$supported _owned_case>])])+
            $(#[bench::$ours(setup = [<$ours _owned_case>])])+
            fn owned(case: BenchmarkCase) {
                case.run();
            }

            #[metabench::benchmark(BORROWED, "http_headers_per_header/per_header", "http_headers_borrowed")]
            $(#[bench::$supported(setup = [<$supported _borrowed_case>])])+
            $(#[bench::$ours(setup = [<$ours _borrowed_case>])])+
            fn borrowed(case: BenchmarkCase) {
                case.run();
            }

            fn criterion_benchmarks(criterion: &mut Criterion) {
                let mut group = criterion.benchmark_group("http_headers_per_header/per_header");
                $(
                    group.bench_function(
                        BenchmarkId::new(COMPETITOR.benchmark_name(), stringify!($supported)),
                        |bencher| {
                            bencher.iter_batched(
                                [<$supported _headers_case>],
                                competitor,
                                BatchSize::SmallInput,
                            );
                        },
                    );
                    group.bench_function(
                        BenchmarkId::new(OWNED.benchmark_name(), stringify!($supported)),
                        |bencher| {
                            bencher.iter_batched(
                                [<$supported _owned_case>],
                                owned,
                                BatchSize::SmallInput,
                            );
                        },
                    );
                    group.bench_function(
                        BenchmarkId::new(BORROWED.benchmark_name(), stringify!($supported)),
                        |bencher| {
                            bencher.iter_batched(
                                [<$supported _borrowed_case>],
                                borrowed,
                                BatchSize::SmallInput,
                            );
                        },
                    );
                )+
                $(
                    group.bench_function(
                        BenchmarkId::new(OWNED.benchmark_name(), stringify!($ours)),
                        |bencher| {
                            bencher.iter_batched(
                                [<$ours _owned_case>],
                                owned,
                                BatchSize::SmallInput,
                            );
                        },
                    );
                    group.bench_function(
                        BenchmarkId::new(BORROWED.benchmark_name(), stringify!($ours)),
                        |bencher| {
                            bencher.iter_batched(
                                [<$ours _borrowed_case>],
                                borrowed,
                                BatchSize::SmallInput,
                            );
                        },
                    );
                )+
                group.finish();
            }

            metabench::main!(
                criterion = {
                    factory = tuned_criterion,
                    benchmarks = criterion_benchmarks,
                    unit = "ns",
                },
                benchmarks = [COMPETITOR, OWNED, BORROWED],
            );
        }
    };
}

finish!(
    supported = [
        accept_ranges,
        access_control_allow_credentials,
        access_control_allow_headers,
        access_control_allow_methods,
        access_control_allow_origin,
        access_control_expose_headers,
        access_control_max_age,
        access_control_request_headers,
        access_control_request_method,
        allow,
        authorization_basic,
        authorization_bearer,
        cache_control,
        content_length,
        content_range,
        content_type,
        etag,
        host,
        if_match,
        if_modified_since,
        if_none_match,
        if_range,
        if_unmodified_since,
        last_modified,
        location,
        range,
        referrer_policy,
        sec_websocket_accept,
        sec_websocket_key,
        sec_websocket_version,
        server,
        set_cookie,
        strict_transport_security,
        user_agent,
        vary,
    ],
    ours = [
        accept,
        accept_encoding,
        accept_language,
        content_security_policy,
        sec_websocket_extensions,
        sec_websocket_protocol,
        sec_websocket_version_advertisement,
        x_content_type_options,
    ],
);
