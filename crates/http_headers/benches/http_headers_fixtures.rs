// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The corpus every `http_headers` benchmark shares with its `headers` twin.
//!
//! Both competitors read byte-identical [`HeaderMap`]s built here, so no
//! comparison degenerates into a comparison of fixtures. Included with
//! `#[path]` rather than reached through the library, because each benchmark
//! file is its own crate and the corpus must not enter the public API.
//!
//! Values are built with [`HeaderValue::from_bytes`] rather than
//! `from_static`, so their storage is heap-backed exactly as it is after a
//! real request is parsed off a socket. That matters: cloning a static value
//! is free, and a fixture of static values would hide every clone the
//! competitor performs.

use http::header::{
    ACCEPT, ACCEPT_ENCODING, ACCEPT_LANGUAGE, AUTHORIZATION, CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST,
    REFERER, USER_AGENT,
};
use http::{HeaderMap, HeaderName, HeaderValue};

#[path = "http_headers_common_values.rs"]
mod common_values;

/// A browser `User-Agent`, long enough to cross the SIMD dispatch threshold.
pub(crate) const USER_AGENT_VALUE: &[u8] =
    b"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// The `Content-Type` of an ordinary JSON API request.
pub(crate) const CONTENT_TYPE_VALUE: &[u8] = b"application/json; charset=utf-8";

/// A `Content-Type` neither crate can parse.
pub(crate) const CONTENT_TYPE_MALFORMED: &[u8] = b"application";

/// A single-line `Cache-Control` with three directives.
pub(crate) const CACHE_CONTROL_VALUE: &[u8] = b"max-age=3600, public, must-revalidate";

/// A directive set delivered as three separate field lines.
pub(crate) const CACHE_CONTROL_LINES: [&[u8]; 3] = [b"max-age=3600", b"public, must-revalidate", b"no-transform, s-maxage=120"];

/// A signed JWT credential of realistic length.
pub(crate) const BEARER_VALUE: &[u8] = b"Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

/// The credential portion of [`BEARER_VALUE`], after the scheme.
pub(crate) const BEARER_TOKEN: &[u8] = b"eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

/// `Basic` credentials for `aladdin:opensesame`.
pub(crate) const BASIC_VALUE: &[u8] = common_values::BASIC_AUTHORIZATION.as_bytes();

/// The username encoded in [`BASIC_VALUE`].
pub(crate) const BASIC_USERNAME: &[u8] = b"aladdin";

/// The password encoded in [`BASIC_VALUE`].
pub(crate) const BASIC_PASSWORD: &[u8] = b"opensesame";

/// Four `Set-Cookie` field lines of the kind a login response emits.
pub(crate) const SET_COOKIE_VALUES: [&[u8]; 4] = [
    b"session=6f1c2a9b4e7d8f3a5c0b1d2e3f4a5b6c; Path=/; HttpOnly; Secure; SameSite=Lax",
    b"csrf=9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d; Path=/; Secure; SameSite=Strict",
    b"theme=dark; Path=/; Max-Age=31536000",
    b"locale=en-US; Path=/; Max-Age=31536000",
];

/// A downstream extension header carried by every request in the corpus.
pub(crate) static REQUEST_ID_NAME: HeaderName = HeaderName::from_static("x-request-id");

/// The value carried by [`REQUEST_ID_NAME`].
pub(crate) const REQUEST_ID_VALUE: &[u8] = b"0f6c9a3e-84d1-4f2b-9c77-2b5e1a0d6f38";

/// The `charset` parameter value both crates look up.
pub(crate) const CHARSET: &[u8] = b"utf-8";

/// Builds a header value, failing loudly rather than measuring a bad fixture.
pub(crate) fn value(bytes: &[u8]) -> HeaderValue {
    HeaderValue::from_bytes(bytes).expect("benchmark fixture is not a legal header value")
}

/// Builds the [`http_headers::FieldValue`] counterpart of [`value`].
pub(crate) fn field_value(bytes: &[u8]) -> http_headers::FieldValue {
    http_headers::FieldValue::from_bytes(bytes).expect("benchmark fixture is not a legal field value")
}

/// Asserts a measured operation produced exactly `expected` and returns its
/// length, so the optimizer cannot delete the work that produced it.
///
/// Both competitors call this identical function on identical bytes, so the
/// comparison it guards costs the same on either side.
pub(crate) fn expect_bytes(actual: &[u8], expected: &[u8]) -> usize {
    assert!(actual == expected, "benchmark produced unexpected bytes");
    actual.len()
}

/// Asserts a measured operation produced exactly `expected`.
pub(crate) fn expect_usize(actual: usize, expected: usize) -> usize {
    assert!(actual == expected, "benchmark produced unexpected count");
    actual
}

/// Consumes a fallible insertion, deliberately without requiring the error to
/// be `Debug`.
///
/// `Field::insert` is fallible because `HeaderMap` has a maximum capacity, and
/// the error types that describe that condition are not all `Debug`:
/// `http`'s own `TryEntryError` explicitly is not. Going through `is_ok`
/// rather than `expect` keeps every call site here compiling whichever error
/// the API settles on, and it is also what a real caller writes: one branch,
/// which is exactly what the measured insertion cases should be paying for.
pub(crate) fn expect_inserted<E>(result: Result<(), E>) -> usize {
    let inserted = match result {
        Ok(()) => 1,
        Err(_full) => 0,
    };
    expect_usize(inserted, 1)
}

/// The headers of an ordinary authenticated JSON API request.
pub(crate) fn json_request() -> HeaderMap {
    let mut map = HeaderMap::with_capacity(16);
    let _ = map.insert(HOST, value(b"api.example.com"));
    let _ = map.insert(USER_AGENT, value(USER_AGENT_VALUE));
    let _ = map.insert(ACCEPT, value(b"application/json, text/plain;q=0.9, */*;q=0.8"));
    let _ = map.insert(ACCEPT_ENCODING, value(b"gzip, deflate, br"));
    let _ = map.insert(ACCEPT_LANGUAGE, value(b"en-US,en;q=0.9"));
    let _ = map.insert(CONTENT_TYPE, value(CONTENT_TYPE_VALUE));
    let _ = map.insert(CONTENT_LENGTH, value(b"348"));
    let _ = map.insert(AUTHORIZATION, value(BEARER_VALUE));
    let _ = map.insert(CACHE_CONTROL, value(CACHE_CONTROL_VALUE));
    let _ = map.insert(COOKIE, value(b"session=6f1c2a9b4e7d8f3a5c0b1d2e3f4a5b6c; theme=dark"));
    let _ = map.insert(&REQUEST_ID_NAME, value(REQUEST_ID_VALUE));
    let _ = map.insert(REFERER, value(b"https://app.example.com/dashboard"));
    let _ = map.insert(CONNECTION, value(b"keep-alive"));
    map
}

/// The same request, with `Authorization` carrying `Basic` credentials.
pub(crate) fn basic_auth_request() -> HeaderMap {
    let mut map = json_request();
    let _ = map.insert(AUTHORIZATION, value(BASIC_VALUE));
    map
}

/// A request whose optional headers are absent or malformed.
///
/// Middleware meets this shape constantly: no `Authorization`, no
/// `Cache-Control`, and a `Content-Type` that fails the grammar.
pub(crate) fn degraded_request() -> HeaderMap {
    let mut map = HeaderMap::with_capacity(16);
    let _ = map.insert(HOST, value(b"api.example.com"));
    let _ = map.insert(USER_AGENT, value(USER_AGENT_VALUE));
    let _ = map.insert(CONTENT_TYPE, value(CONTENT_TYPE_MALFORMED));
    let _ = map.insert(CONTENT_LENGTH, value(b"0"));
    let _ = map.insert(&REQUEST_ID_NAME, value(REQUEST_ID_VALUE));
    map
}

/// A map whose `Cache-Control` arrives as [`CACHE_CONTROL_LINES`].
pub(crate) fn cache_control_multi_line() -> HeaderMap {
    let mut map = json_request();
    let _ = map.remove(CACHE_CONTROL);
    for line in CACHE_CONTROL_LINES {
        let _ = map.append(CACHE_CONTROL, value(line));
    }
    map
}

/// A legal but adversarial request: every value sits at the large end of what
/// a gateway accepts, and none of it is invalid.
pub(crate) fn adversarial_request() -> HeaderMap {
    let mut map = HeaderMap::with_capacity(8);
    let mut agent = Vec::with_capacity(4096);
    while agent.len() < 4000 {
        agent.extend_from_slice(b"Component/1.0 (build 20240101; feature-set-extended) ");
    }
    agent.truncate(4000);
    let _ = map.insert(USER_AGENT, value(&agent));

    let mut content_type = Vec::with_capacity(2048);
    content_type.extend_from_slice(b"application/vnd.example.v3+json");
    for index in 0..48 {
        content_type.extend_from_slice(format!("; param{index}=value{index}").as_bytes());
    }
    content_type.extend_from_slice(b"; charset=utf-8");
    let _ = map.insert(CONTENT_TYPE, value(&content_type));

    let mut cache_control = Vec::with_capacity(1024);
    cache_control.extend_from_slice(b"max-age=3600");
    for index in 0..32 {
        cache_control.extend_from_slice(format!(", ext{index}=value{index}").as_bytes());
    }
    cache_control.extend_from_slice(b", public");
    let _ = map.insert(CACHE_CONTROL, value(&cache_control));

    let _ = map.insert(AUTHORIZATION, value(BEARER_VALUE));
    let _ = map.insert(&REQUEST_ID_NAME, value(REQUEST_ID_VALUE));
    map
}
