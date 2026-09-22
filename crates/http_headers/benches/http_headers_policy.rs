// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Focused policy-header instruction-count experiments.

#![expect(
    clippy::unwrap_used,
    reason = "fixed benchmark fixtures are asserted valid outside the measured operations"
)]

use std::hint::black_box;
use std::sync::OnceLock;

use criterion::{BatchSize, Criterion};
use http::{HeaderMap, HeaderValue};
use http_headers::headers::{
    Authorization, AuthorizationOwned, Basic, BasicCredentials, CacheControl, ContentSecurityPolicy, Location, ReferrerPolicy, SetCookie,
    StrictTransportSecurity,
};
use http_headers::{DecodeMode, Field};

fn map(name: &'static str, values: &[&'static str]) -> HeaderMap {
    let mut map = HeaderMap::with_capacity(1);
    for value in values {
        map.append(name, HeaderValue::from_static(value));
    }
    map
}

fn referrer_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| {
        map(
            "referrer-policy",
            &["future-policy, no-referrer", "origin, strict-origin-when-cross-origin"],
        )
    })
}

fn cache_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| {
        map(
            "cache-control",
            &["public, max-age=31536000, stale-while-revalidate=60, x-build=123456789012345678901"],
        )
    })
}

fn hsts_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| map("strict-transport-security", &["MAX-AGE=31536000; includeSubDomains; preload"]))
}

fn location_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| map("location", &["https://example.com/a%20path?query=1#fragment"]))
}

fn repeated_csp_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| {
        map(
            "content-security-policy",
            &[
                "default-src 'self'",
                "script-src 'none'",
                "img-src https:",
                "style-src 'self'",
                "font-src https:",
                "connect-src 'self'",
                "frame-src 'none'",
                "object-src 'none'",
                "base-uri 'self'",
                "form-action 'self'",
                "frame-ancestors 'none'",
                "upgrade-insecure-requests",
                "block-all-mixed-content",
                "worker-src 'self'",
                "manifest-src 'self'",
                "media-src 'none'",
            ],
        )
    })
}

fn repeated_cookie_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| {
        map(
            "set-cookie",
            &[
                "a=1", "b=2", "c=3", "d=4", "e=5", "f=6", "g=7", "h=8", "i=9", "j=10", "k=11", "l=12", "m=13", "n=14", "o=15", "p=16",
            ],
        )
    })
}

fn basic() -> AuthorizationOwned<Basic> {
    AuthorizationOwned::<Basic>::basic(b"benchmark-user", b"benchmark-password").unwrap()
}

fn authorization_map() -> &'static HeaderMap {
    static MAP: OnceLock<HeaderMap> = OnceLock::new();
    MAP.get_or_init(|| map("authorization", &["Basic YmVuY2htYXJrLXVzZXI6YmVuY2htYXJrLXBhc3N3b3Jk"]))
}

fn basic_and_credentials() -> (AuthorizationOwned<Basic>, BasicCredentials) {
    (basic(), BasicCredentials::new())
}

fn drop_it<T>(value: T) {
    drop(value);
}

#[metabench::benchmark(REFERRER_DECODE, "policy", "referrer_decode", gungraun_setup = referrer_map)]
fn referrer_decode(map: &'static HeaderMap) -> usize {
    let value = ReferrerPolicy::view(black_box(map)).unwrap().unwrap();
    black_box(value.policies().count())
}

#[metabench::benchmark(REFERRER_PREFERRED_8, "policy", "referrer_preferred_8", gungraun_setup = referrer_map)]
fn referrer_preferred_8(map: &'static HeaderMap) -> usize {
    let value = ReferrerPolicy::view(black_box(map)).unwrap().unwrap();
    let mut result = 0;
    for _ in 0..8 {
        result ^= value.preferred().unwrap() as usize;
    }
    black_box(result)
}

#[metabench::benchmark(CACHE_DIRECTIVES, "policy", "cache_directives", gungraun_setup = cache_map)]
fn cache_directives(map: &'static HeaderMap) -> usize {
    let value = CacheControl::view(black_box(map)).unwrap().unwrap();
    black_box(value.directives().map(|directive| directive.as_bytes().len()).sum())
}

#[metabench::benchmark(CACHE_MAX_AGE_8, "policy", "cache_max_age_8", gungraun_setup = cache_map)]
fn cache_max_age_8(map: &'static HeaderMap) -> u64 {
    let value = CacheControl::view(black_box(map)).unwrap().unwrap();
    let mut result = 0;
    for _ in 0..8 {
        result ^= value.max_age().unwrap().as_secs();
    }
    black_box(result)
}

#[metabench::benchmark(HSTS_DECODE, "policy", "hsts_decode", gungraun_setup = hsts_map)]
fn hsts_decode(map: &'static HeaderMap) -> u64 {
    let value = StrictTransportSecurity::view(black_box(map)).unwrap().unwrap();
    black_box(value.max_age().as_secs())
}

#[metabench::benchmark(HSTS_DIRECTIVES, "policy", "hsts_directives", gungraun_setup = hsts_map)]
fn hsts_directives(map: &'static HeaderMap) -> usize {
    let value = StrictTransportSecurity::view(black_box(map)).unwrap().unwrap();
    black_box(value.directives().map(|directive| directive.unwrap().as_bytes().len()).sum())
}

#[metabench::benchmark(AUTH_ACCESS_8, "policy", "auth_access_8", gungraun_setup = basic, gungraun_teardown = drop_it)]
fn auth_access_8(value: AuthorizationOwned<Basic>) -> (usize, AuthorizationOwned<Basic>) {
    let mut result = 0;
    for _ in 0..8 {
        result ^= value.encoded_credentials().unwrap().len();
    }
    (black_box(result), value)
}

#[metabench::benchmark(AUTH_DECODE, "policy", "auth_decode", gungraun_setup = authorization_map)]
fn auth_decode(map: &'static HeaderMap) -> usize {
    let value = Authorization::<Basic>::owned(black_box(map)).unwrap().unwrap();
    black_box(value.encoded_credentials().unwrap().len())
}

#[metabench::benchmark(
    AUTH_EXTRACT_WARM,
    "policy",
    "auth_extract_warm",
    gungraun_setup = basic_and_credentials,
    gungraun_teardown = drop_it,
)]
fn auth_extract_warm(state: (AuthorizationOwned<Basic>, BasicCredentials)) -> (usize, (AuthorizationOwned<Basic>, BasicCredentials)) {
    let (value, mut credentials) = state;
    value.extract(&mut credentials).unwrap();
    let result = value.extract(&mut credentials).unwrap().username().len();
    (black_box(result), (value, credentials))
}

#[metabench::benchmark(CSP_FROM_BYTES, "policy", "csp_from_bytes")]
fn csp_from_bytes() -> usize {
    let value =
        http_headers::headers::ContentSecurityPolicyOwned::from_bytes(black_box(b"default-src 'self'; script-src 'nonce-abcdefghijklmno'"))
            .unwrap();
    black_box(value.policies().next().unwrap().len())
}

#[metabench::benchmark(CSP_OWNED_16, "policy", "csp_owned_16", gungraun_setup = repeated_csp_map)]
fn csp_owned_16(map: &'static HeaderMap) -> usize {
    let value = ContentSecurityPolicy::owned(black_box(map)).unwrap().unwrap();
    black_box(value.policies().count())
}

#[metabench::benchmark(SET_COOKIE_OWNED_16, "policy", "set_cookie_owned_16", gungraun_setup = repeated_cookie_map)]
fn set_cookie_owned_16(map: &'static HeaderMap) -> usize {
    let value = SetCookie::owned(black_box(map)).unwrap().unwrap();
    black_box(value.len())
}

#[metabench::benchmark(LOCATION_RELAXED, "policy", "location_relaxed", gungraun_setup = location_map)]
fn location_relaxed(map: &'static HeaderMap) -> usize {
    let value = Location::view_with(black_box(map), DecodeMode::Relaxed).unwrap().unwrap();
    black_box(value.as_bytes().len())
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("http_headers_policy/policy");
    macro_rules! with_setup {
        ($id:ident, $setup:ident, $function:ident) => {
            group.bench_function($id.benchmark_name(), |bencher| {
                bencher.iter_batched($setup, $function, BatchSize::SmallInput);
            });
        };
    }
    with_setup!(REFERRER_DECODE, referrer_map, referrer_decode);
    with_setup!(REFERRER_PREFERRED_8, referrer_map, referrer_preferred_8);
    with_setup!(CACHE_DIRECTIVES, cache_map, cache_directives);
    with_setup!(CACHE_MAX_AGE_8, cache_map, cache_max_age_8);
    with_setup!(HSTS_DECODE, hsts_map, hsts_decode);
    with_setup!(HSTS_DIRECTIVES, hsts_map, hsts_directives);
    with_setup!(AUTH_ACCESS_8, basic, auth_access_8);
    with_setup!(AUTH_DECODE, authorization_map, auth_decode);
    with_setup!(AUTH_EXTRACT_WARM, basic_and_credentials, auth_extract_warm);
    group.bench_function(CSP_FROM_BYTES.benchmark_name(), |b| b.iter(csp_from_bytes));
    with_setup!(CSP_OWNED_16, repeated_csp_map, csp_owned_16);
    with_setup!(SET_COOKIE_OWNED_16, repeated_cookie_map, set_cookie_owned_16);
    with_setup!(LOCATION_RELAXED, location_map, location_relaxed);
    group.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        REFERRER_DECODE,
        REFERRER_PREFERRED_8,
        CACHE_DIRECTIVES,
        CACHE_MAX_AGE_8,
        HSTS_DECODE,
        HSTS_DIRECTIVES,
        AUTH_ACCESS_8,
        AUTH_DECODE,
        AUTH_EXTRACT_WARM,
        CSP_FROM_BYTES,
        CSP_OWNED_16,
        SET_COOKIE_OWNED_16,
        LOCATION_RELAXED,
    ],
);
