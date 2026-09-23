// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Unified microbenchmarks for `http_headers` beside `headers`.
//!
//! # How a pair is formed
//!
//! Every case is two functions sharing one `#[bench]` id, in a group carrying
//! `compare_by_id = true`, so each engine reports the pair under one case id.
//! The function name is the id followed by the arm:
//! `user_agent_borrowed_http_headers` and
//! `user_agent_borrowed_headers` are the two arms of `user_agent_borrowed`.
//!
//! Giving every case its own function, rather than one function switching on
//! an argument, keeps the dispatch out of the measured region: what Callgrind
//! attributes to a case is the operation and nothing else.
//!
//! # Fairness
//!
//! Both arms of a pair receive the same prebuilt [`HeaderMap`] from a setup
//! function, and setup and teardown run outside the measured region. Both end
//! by calling the same `expect_bytes`/`expect_usize` assertion against the same
//! expected answer, so a decoder that skipped work fails instead of posting a
//! better number.
//!
//! Where the crates cannot do the same thing the case says so rather than
//! inventing an equivalence. `headers` has no borrowed view, so
//! `user_agent_borrowed` races a borrowed lookup against an owned one and is
//! published as a capability difference; `user_agent_owned` is the
//! like-for-like row. `headers::SetCookie` exposes no accessor at all, so its
//! arm can only decode where ours decodes *and* reads every cookie.
//!
//! # Reading these numbers
//!
//! Metabench combines Criterion time, Gungraun instruction counts, and
//! allocation measurements. Results are comparable within this binary and not
//! across files, because the optimizer's inlining decisions inside a measured
//! region depend on the rest of the binary.

use std::hint::black_box;
use std::str::{self, FromStr as _};
use std::sync::LazyLock;
use std::time::Duration;

use criterion::measurement::WallTime;
use criterion::{BatchSize, BenchmarkGroup, BenchmarkId, Criterion};
use headers::HeaderMapExt as TheirMapExt;
use http::HeaderMap;
use http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, SET_COOKIE, USER_AGENT};
use http_headers::headers::{
    Authorization, AuthorizationOwned, Basic, BasicCredentials, Bearer, CacheControl, CacheControlOwned, ContentType, ContentTypeOwned,
    ContentTypeView, SetCookie, SetCookieOwned, UserAgent, UserAgentOwned,
};
use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldValue, FieldValueRef};

#[path = "http_headers_fixtures.rs"]
mod fixtures;

use fixtures::{
    BASIC_PASSWORD, BASIC_USERNAME, BEARER_TOKEN, CHARSET, SET_COOKIE_VALUES, USER_AGENT_VALUE, expect_bytes, expect_inserted,
    expect_usize, field_value, value,
};

// ── corpus, all of it built outside every measured region ────────────────────

/// A `Content-Type` carrying several parameters, one of them quoted.
const CONTENT_TYPE_PARAMETERS: &[u8] = b"multipart/form-data; boundary=------------------------1a2b3c; charset=utf-8; name=\"upload\"";

/// The base64 payload of the corpus `Basic` credentials.
const BASIC_ENCODED: &[u8] = b"YWxhZGRpbjpvcGVuc2VzYW1l";

/// `boundary` + `charset` + `name`, the parameter names both crates report.
const PARAMETER_NAME_BYTES: usize = 19;

const COOKIE_BYTES_1: usize = SET_COOKIE_VALUES[0].len();
const COOKIE_BYTES_4: usize =
    SET_COOKIE_VALUES[0].len() + SET_COOKIE_VALUES[1].len() + SET_COOKIE_VALUES[2].len() + SET_COOKIE_VALUES[3].len();
const COOKIE_BYTES_12: usize = 3 * COOKIE_BYTES_4;

const MAX_AGE: Duration = Duration::from_hours(1);
const CREDENTIAL_ROUNDS: usize = 8;

static JSON_REQUEST: LazyLock<HeaderMap> = LazyLock::new(fixtures::json_request);
static BASIC_REQUEST: LazyLock<HeaderMap> = LazyLock::new(fixtures::basic_auth_request);
static DEGRADED_REQUEST: LazyLock<HeaderMap> = LazyLock::new(fixtures::degraded_request);
static CACHE_CONTROL_MULTI: LazyLock<HeaderMap> = LazyLock::new(fixtures::cache_control_multi_line);
static ADVERSARIAL_REQUEST: LazyLock<HeaderMap> = LazyLock::new(fixtures::adversarial_request);
static COOKIES_1: LazyLock<HeaderMap> = LazyLock::new(|| cookies(1));
static COOKIES_4: LazyLock<HeaderMap> = LazyLock::new(|| cookies(4));
static COOKIES_12: LazyLock<HeaderMap> = LazyLock::new(|| cookies(12));
static CONTENT_TYPE_MANY: LazyLock<HeaderMap> = LazyLock::new(|| {
    let mut map = HeaderMap::with_capacity(2);
    let _ = map.insert(CONTENT_TYPE, value(CONTENT_TYPE_PARAMETERS));
    map
});
static LARGE_BASIC: LazyLock<HeaderMap> = LazyLock::new(large_basic_map);
/// One directive, so the `list` rows span 1, 3, and 34 and the fixed part of a
/// decode can be separated from its per-directive part by measurement.
static CACHE_CONTROL_ONE: LazyLock<HeaderMap> = LazyLock::new(|| {
    let mut map = HeaderMap::with_capacity(2);
    let _ = map.insert(CACHE_CONTROL, value(b"max-age=3600"));
    map
});

fn cookies(count: usize) -> HeaderMap {
    let mut map = HeaderMap::with_capacity(count.next_power_of_two());
    for index in 0..count {
        let _ = map.append(SET_COOKIE, value(SET_COOKIE_VALUES[index % 4]));
    }
    map
}

fn large_basic_map() -> HeaderMap {
    let mut password = Vec::with_capacity(4096);
    while password.len() < 2976 {
        password.extend_from_slice(b"0123456789abcdef");
    }
    password.truncate(2976);
    let credentials = AuthorizationOwned::<Basic>::basic(b"gateway-service-account", &password).expect("legal credentials");
    let mut map = empty_map();
    expect_inserted(Authorization::<Basic>::insert(&mut map, credentials));
    map
}

// ── shared setup ─────────────────────────────────────────────────────────────

fn json_map() -> &'static HeaderMap {
    &JSON_REQUEST
}

fn bearer_map() -> &'static HeaderMap {
    black_box(http_headers_simd::is_token68(BEARER_TOKEN));
    &JSON_REQUEST
}

fn basic_map() -> &'static HeaderMap {
    &BASIC_REQUEST
}

fn degraded_map() -> &'static HeaderMap {
    &DEGRADED_REQUEST
}

fn cache_one_map() -> &'static HeaderMap {
    &CACHE_CONTROL_ONE
}

fn cache_multi_map() -> &'static HeaderMap {
    &CACHE_CONTROL_MULTI
}

fn adversarial_map() -> &'static HeaderMap {
    &ADVERSARIAL_REQUEST
}

fn content_type_many_map() -> &'static HeaderMap {
    &CONTENT_TYPE_MANY
}

fn cookies_1() -> &'static HeaderMap {
    &COOKIES_1
}

fn cookies_4() -> &'static HeaderMap {
    &COOKIES_4
}

fn cookies_12() -> &'static HeaderMap {
    &COOKIES_12
}

fn large_map() -> &'static HeaderMap {
    &LARGE_BASIC
}

fn empty_map() -> HeaderMap {
    HeaderMap::with_capacity(8)
}

fn drop_it<T>(value: T) {
    drop(value);
}

/// Keeps an owned result alive past the measured region without letting the
/// optimizer delete the work that produced it.
fn consume<T>(value: T) {
    drop(black_box(value));
}

// ── group: lookup ────────────────────────────────────────────────────────────

#[metabench::benchmark(
    USER_AGENT_BORROWED_HTTP_HEADERS,
    "lookup",
    "user_agent_borrowed_http_headers",
    gungraun_setup = json_map,
)]
#[bench::user_agent_borrowed()]
fn user_agent_borrowed_http_headers(map: &'static HeaderMap) -> usize {
    let view = UserAgent::view(map).expect("valid user agent").expect("present user agent");
    expect_bytes(view.as_bytes(), USER_AGENT_VALUE)
}

#[metabench::benchmark(
    USER_AGENT_BORROWED_HEADERS,
    "lookup",
    "user_agent_borrowed_headers",
    gungraun_setup = json_map,
)]
#[bench::user_agent_borrowed()]
fn user_agent_borrowed_headers(map: &'static HeaderMap) -> usize {
    let agent = TheirMapExt::typed_try_get::<headers::UserAgent>(map)
        .expect("valid user agent")
        .expect("present user agent");
    let length = expect_bytes(agent.as_str().as_bytes(), USER_AGENT_VALUE);
    consume(agent);
    length
}

#[metabench::benchmark(
    USER_AGENT_OWNED_HTTP_HEADERS,
    "lookup",
    "user_agent_owned_http_headers",
    gungraun_setup = json_map,
)]
#[bench::user_agent_owned()]
fn user_agent_owned_http_headers(map: &'static HeaderMap) -> usize {
    let agent = UserAgent::owned(map).expect("valid user agent").expect("present user agent");
    let length = expect_bytes(agent.as_bytes(), USER_AGENT_VALUE);
    consume(agent);
    length
}

#[metabench::benchmark(
    USER_AGENT_OWNED_HEADERS,
    "lookup",
    "user_agent_owned_headers",
    gungraun_setup = json_map,
)]
#[bench::user_agent_owned()]
fn user_agent_owned_headers(map: &'static HeaderMap) -> usize {
    let agent = TheirMapExt::typed_try_get::<headers::UserAgent>(map)
        .expect("valid user agent")
        .expect("present user agent");
    let length = expect_bytes(agent.as_str().as_bytes(), USER_AGENT_VALUE);
    consume(agent);
    length
}

#[metabench::benchmark(
    USER_AGENT_ABSENT_HTTP_HEADERS,
    "lookup",
    "user_agent_absent_http_headers",
    gungraun_setup = cookies_4,
)]
#[bench::user_agent_absent()]
fn user_agent_absent_http_headers(map: &'static HeaderMap) -> usize {
    let absent = UserAgent::view(map).expect("absent is not an error");
    expect_usize(usize::from(absent.is_none()), 1)
}

#[metabench::benchmark(
    USER_AGENT_ABSENT_HEADERS,
    "lookup",
    "user_agent_absent_headers",
    gungraun_setup = cookies_4,
)]
#[bench::user_agent_absent()]
fn user_agent_absent_headers(map: &'static HeaderMap) -> usize {
    let absent = TheirMapExt::typed_try_get::<headers::UserAgent>(map).expect("absent is not an error");
    expect_usize(usize::from(absent.is_none()), 1)
}

// ── group: repeated ──────────────────────────────────────────────────────────

fn read_cookies(map: &HeaderMap, expected: usize) -> usize {
    let view = SetCookie::view(map).expect("valid cookies").expect("present cookies");
    let mut total = 0;
    for cookie in view.iter() {
        total += cookie.as_bytes().len();
    }
    expect_usize(total, expected)
}

/// `headers::SetCookie` has no accessor, so its only measurable operation is
/// the decode, which clones every field value into a `Vec`.
fn decode_cookies(map: &HeaderMap) -> usize {
    let cookies = TheirMapExt::typed_try_get::<headers::SetCookie>(map)
        .expect("valid cookies")
        .expect("present cookies");
    consume(cookies);
    1
}

#[metabench::benchmark(
    SET_COOKIE_BORROWED_1_HTTP_HEADERS,
    "repeated",
    "set_cookie_borrowed_1_http_headers",
    gungraun_setup = cookies_1,
)]
#[bench::set_cookie_borrowed_1()]
fn set_cookie_borrowed_1_http_headers(map: &'static HeaderMap) -> usize {
    read_cookies(map, COOKIE_BYTES_1)
}

#[metabench::benchmark(
    SET_COOKIE_BORROWED_1_HEADERS,
    "repeated",
    "set_cookie_borrowed_1_headers",
    gungraun_setup = cookies_1,
)]
#[bench::set_cookie_borrowed_1()]
fn set_cookie_borrowed_1_headers(map: &'static HeaderMap) -> usize {
    decode_cookies(map)
}

#[metabench::benchmark(
    SET_COOKIE_BORROWED_4_HTTP_HEADERS,
    "repeated",
    "set_cookie_borrowed_4_http_headers",
    gungraun_setup = cookies_4,
)]
#[bench::set_cookie_borrowed_4()]
fn set_cookie_borrowed_4_http_headers(map: &'static HeaderMap) -> usize {
    read_cookies(map, COOKIE_BYTES_4)
}

#[metabench::benchmark(
    SET_COOKIE_BORROWED_4_HEADERS,
    "repeated",
    "set_cookie_borrowed_4_headers",
    gungraun_setup = cookies_4,
)]
#[bench::set_cookie_borrowed_4()]
fn set_cookie_borrowed_4_headers(map: &'static HeaderMap) -> usize {
    decode_cookies(map)
}

#[metabench::benchmark(
    SET_COOKIE_BORROWED_12_HTTP_HEADERS,
    "repeated",
    "set_cookie_borrowed_12_http_headers",
    gungraun_setup = cookies_12,
)]
#[bench::set_cookie_borrowed_12()]
fn set_cookie_borrowed_12_http_headers(map: &'static HeaderMap) -> usize {
    read_cookies(map, COOKIE_BYTES_12)
}

#[metabench::benchmark(
    SET_COOKIE_BORROWED_12_HEADERS,
    "repeated",
    "set_cookie_borrowed_12_headers",
    gungraun_setup = cookies_12,
)]
#[bench::set_cookie_borrowed_12()]
fn set_cookie_borrowed_12_headers(map: &'static HeaderMap) -> usize {
    decode_cookies(map)
}

#[metabench::benchmark(
    SET_COOKIE_OWNED_4_HTTP_HEADERS,
    "repeated",
    "set_cookie_owned_4_http_headers",
    gungraun_setup = cookies_4,
)]
#[bench::set_cookie_owned_4()]
fn set_cookie_owned_4_http_headers(map: &'static HeaderMap) -> usize {
    let cookies = SetCookie::owned(map).expect("valid cookies").expect("present cookies");
    let mut total = 0;
    for cookie in &cookies {
        total += cookie.as_bytes().len();
    }
    let total = expect_usize(total, COOKIE_BYTES_4);
    consume(cookies);
    total
}

#[metabench::benchmark(
    SET_COOKIE_OWNED_4_HEADERS,
    "repeated",
    "set_cookie_owned_4_headers",
    gungraun_setup = cookies_4,
)]
#[bench::set_cookie_owned_4()]
fn set_cookie_owned_4_headers(map: &'static HeaderMap) -> usize {
    decode_cookies(map)
}

// ── group: list ──────────────────────────────────────────────────────────────

#[metabench::benchmark(
    CACHE_CONTROL_SINGLE_LINE_HTTP_HEADERS,
    "list",
    "cache_control_single_line_http_headers",
    gungraun_setup = json_map,
)]
#[bench::cache_control_single_line()]
fn cache_control_single_line_http_headers(map: &'static HeaderMap) -> usize {
    let view = CacheControl::view(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = usize::from(view.max_age() == Some(MAX_AGE)) + usize::from(!view.no_cache());
    expect_usize(answer, 2)
}

#[metabench::benchmark(
    CACHE_CONTROL_SINGLE_LINE_HEADERS,
    "list",
    "cache_control_single_line_headers",
    gungraun_setup = json_map,
)]
#[bench::cache_control_single_line()]
fn cache_control_single_line_headers(map: &'static HeaderMap) -> usize {
    let control = TheirMapExt::typed_try_get::<headers::CacheControl>(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = usize::from(control.max_age() == Some(MAX_AGE)) + usize::from(!control.no_cache());
    let answer = expect_usize(answer, 2);
    consume(control);
    answer
}

#[metabench::benchmark(
    CACHE_CONTROL_MULTI_LINE_HTTP_HEADERS,
    "list",
    "cache_control_multi_line_http_headers",
    gungraun_setup = cache_multi_map,
)]
#[bench::cache_control_multi_line()]
fn cache_control_multi_line_http_headers(map: &'static HeaderMap) -> usize {
    let view = CacheControl::view(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = usize::from(view.max_age() == Some(MAX_AGE)) + view.directives().count();
    expect_usize(answer, 6)
}

#[metabench::benchmark(
    CACHE_CONTROL_MULTI_LINE_HEADERS,
    "list",
    "cache_control_multi_line_headers",
    gungraun_setup = cache_multi_map,
)]
#[bench::cache_control_multi_line()]
fn cache_control_multi_line_headers(map: &'static HeaderMap) -> usize {
    let control = TheirMapExt::typed_try_get::<headers::CacheControl>(map)
        .expect("valid cache control")
        .expect("present cache control");
    let recognized = usize::from(control.public())
        + usize::from(control.must_revalidate())
        + usize::from(control.no_transform())
        + usize::from(control.s_max_age().is_some())
        + usize::from(control.max_age().is_some());
    let answer = usize::from(control.max_age() == Some(MAX_AGE)) + recognized;
    let answer = expect_usize(answer, 6);
    consume(control);
    answer
}

#[metabench::benchmark(
    CACHE_CONTROL_OWNED_HTTP_HEADERS,
    "list",
    "cache_control_owned_http_headers",
    gungraun_setup = json_map,
)]
#[bench::cache_control_owned()]
fn cache_control_owned_http_headers(map: &'static HeaderMap) -> usize {
    let control = CacheControl::owned(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = usize::from(control.max_age() == Some(MAX_AGE)) + usize::from(!control.no_cache());
    let answer = expect_usize(answer, 2);
    consume(control);
    answer
}

#[metabench::benchmark(
    CACHE_CONTROL_OWNED_HEADERS,
    "list",
    "cache_control_owned_headers",
    gungraun_setup = json_map,
)]
#[bench::cache_control_owned()]
fn cache_control_owned_headers(map: &'static HeaderMap) -> usize {
    let control = TheirMapExt::typed_try_get::<headers::CacheControl>(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = usize::from(control.max_age() == Some(MAX_AGE)) + usize::from(!control.no_cache());
    let answer = expect_usize(answer, 2);
    consume(control);
    answer
}

#[metabench::benchmark(
    CACHE_CONTROL_ADVERSARIAL_HTTP_HEADERS,
    "list",
    "cache_control_adversarial_http_headers",
    gungraun_setup = adversarial_map,
)]
#[bench::cache_control_adversarial()]
fn cache_control_adversarial_http_headers(map: &'static HeaderMap) -> usize {
    let view = CacheControl::view(map)
        .expect("valid cache control")
        .expect("present cache control");
    expect_usize(usize::from(view.max_age() == Some(MAX_AGE)), 1)
}

#[metabench::benchmark(
    CACHE_CONTROL_ADVERSARIAL_HEADERS,
    "list",
    "cache_control_adversarial_headers",
    gungraun_setup = adversarial_map,
)]
#[bench::cache_control_adversarial()]
fn cache_control_adversarial_headers(map: &'static HeaderMap) -> usize {
    let control = TheirMapExt::typed_try_get::<headers::CacheControl>(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = expect_usize(usize::from(control.max_age() == Some(MAX_AGE)), 1);
    consume(control);
    answer
}

#[metabench::benchmark(
    CACHE_CONTROL_ONE_DIRECTIVE_HTTP_HEADERS,
    "list",
    "cache_control_one_directive_http_headers",
    gungraun_setup = cache_one_map,
)]
#[bench::cache_control_one_directive()]
fn cache_control_one_directive_http_headers(map: &'static HeaderMap) -> usize {
    let view = CacheControl::view(map)
        .expect("valid cache control")
        .expect("present cache control");
    expect_usize(usize::from(view.max_age() == Some(MAX_AGE)), 1)
}

#[metabench::benchmark(
    CACHE_CONTROL_ONE_DIRECTIVE_HEADERS,
    "list",
    "cache_control_one_directive_headers",
    gungraun_setup = cache_one_map,
)]
#[bench::cache_control_one_directive()]
fn cache_control_one_directive_headers(map: &'static HeaderMap) -> usize {
    let control = TheirMapExt::typed_try_get::<headers::CacheControl>(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = expect_usize(usize::from(control.max_age() == Some(MAX_AGE)), 1);
    consume(control);
    answer
}

#[metabench::benchmark(
    CACHE_CONTROL_MAX_AGE_ONLY_HTTP_HEADERS,
    "list",
    "cache_control_max_age_only_http_headers",
    gungraun_setup = json_map,
)]
#[bench::cache_control_max_age_only()]
fn cache_control_max_age_only_http_headers(map: &'static HeaderMap) -> usize {
    let view = CacheControl::view(map)
        .expect("valid cache control")
        .expect("present cache control");
    expect_usize(usize::from(view.max_age() == Some(MAX_AGE)), 1)
}

#[metabench::benchmark(
    CACHE_CONTROL_MAX_AGE_ONLY_HEADERS,
    "list",
    "cache_control_max_age_only_headers",
    gungraun_setup = json_map,
)]
#[bench::cache_control_max_age_only()]
fn cache_control_max_age_only_headers(map: &'static HeaderMap) -> usize {
    let control = TheirMapExt::typed_try_get::<headers::CacheControl>(map)
        .expect("valid cache control")
        .expect("present cache control");
    let answer = expect_usize(usize::from(control.max_age() == Some(MAX_AGE)), 1);
    consume(control);
    answer
}

// ── group: directives ────────────────────────────────────────────────────────

/// `max-age=3600` is the only directive in the single-line corpus with a value.
const SINGLE_LINE_VALUE_BYTES: usize = 4;

/// One `max-age` plus 32 `ext<n>=value<n>` directives in the adversarial corpus.
const ADVERSARIAL_VALUE_BYTES: usize = 218;

fn sum_value_bytes(map: &HeaderMap, expected: usize) -> usize {
    let view = CacheControl::view(map)
        .expect("valid cache control")
        .expect("present cache control");
    let mut total = 0;
    for directive in view.directives() {
        total += directive.value().map_or(0, <[u8]>::len);
    }
    expect_usize(total, expected)
}

fn sum_value_str(map: &HeaderMap, expected: usize) -> usize {
    let view = CacheControl::view(map)
        .expect("valid cache control")
        .expect("present cache control");
    let mut total = 0;
    for directive in view.directives() {
        total += directive.value_str().expect("utf-8 directive value").map_or(0, str::len);
    }
    expect_usize(total, expected)
}

#[metabench::benchmark(
    DIRECTIVE_VALUE_BYTES,
    "directives",
    "directive_value_bytes",
    gungraun_setup = json_map,
)]
#[bench::directive_value()]
fn directive_value_bytes(map: &'static HeaderMap) -> usize {
    sum_value_bytes(map, SINGLE_LINE_VALUE_BYTES)
}

#[metabench::benchmark(
    DIRECTIVE_VALUE_STR,
    "directives",
    "directive_value_str",
    gungraun_setup = json_map,
)]
#[bench::directive_value()]
fn directive_value_str(map: &'static HeaderMap) -> usize {
    sum_value_str(map, SINGLE_LINE_VALUE_BYTES)
}

#[metabench::benchmark(
    DIRECTIVE_VALUE_MANY_BYTES,
    "directives",
    "directive_value_many_bytes",
    gungraun_setup = adversarial_map,
)]
#[bench::directive_value_many()]
fn directive_value_many_bytes(map: &'static HeaderMap) -> usize {
    sum_value_bytes(map, ADVERSARIAL_VALUE_BYTES)
}

#[metabench::benchmark(
    DIRECTIVE_VALUE_MANY_STR,
    "directives",
    "directive_value_many_str",
    gungraun_setup = adversarial_map,
)]
#[bench::directive_value_many()]
fn directive_value_many_str(map: &'static HeaderMap) -> usize {
    sum_value_str(map, ADVERSARIAL_VALUE_BYTES)
}

// ── group: structured ────────────────────────────────────────────────────────

fn inspect_content_type(view: &ContentTypeView<'_>) -> usize {
    let mut total = expect_bytes(view.type_().expect("utf-8 type").as_bytes(), b"application");
    total += expect_bytes(view.subtype().expect("utf-8 subtype").as_bytes(), b"json");
    total += expect_bytes(
        view.parameter("charset").expect("valid parameters").expect("charset present"),
        CHARSET,
    );
    expect_usize(total, 20)
}

fn inspect_mime(mime: &headers::Mime) -> usize {
    let mut total = expect_bytes(mime.type_().as_str().as_bytes(), b"application");
    total += expect_bytes(mime.subtype().as_str().as_bytes(), b"json");
    total += expect_bytes(mime.get_param("charset").expect("charset present").as_str().as_bytes(), CHARSET);
    expect_usize(total, 20)
}

#[metabench::benchmark(
    CONTENT_TYPE_INSPECT_HTTP_HEADERS,
    "structured",
    "content_type_inspect_http_headers",
    gungraun_setup = json_map,
)]
#[bench::content_type_inspect()]
fn content_type_inspect_http_headers(map: &'static HeaderMap) -> usize {
    let view = ContentType::view(map).expect("valid content type").expect("present content type");
    inspect_content_type(&view)
}

#[metabench::benchmark(
    CONTENT_TYPE_INSPECT_HEADERS,
    "structured",
    "content_type_inspect_headers",
    gungraun_setup = json_map,
)]
#[bench::content_type_inspect()]
fn content_type_inspect_headers(map: &'static HeaderMap) -> usize {
    let content_type = TheirMapExt::typed_try_get::<headers::ContentType>(map)
        .expect("valid content type")
        .expect("present content type");
    let mime = headers::Mime::from(content_type);
    let total = inspect_mime(&mime);
    consume(mime);
    total
}

#[metabench::benchmark(
    CONTENT_TYPE_PARAMETERS_HTTP_HEADERS,
    "structured",
    "content_type_parameters_http_headers",
    gungraun_setup = content_type_many_map,
)]
#[bench::content_type_parameters()]
fn content_type_parameters_http_headers(map: &'static HeaderMap) -> usize {
    let view = ContentType::view(map).expect("valid content type").expect("present content type");
    let mut total = 0;
    for parameter in view.parameters() {
        total += parameter.expect("valid parameter").name().len();
    }
    let total = expect_usize(total, PARAMETER_NAME_BYTES);
    total
        + expect_bytes(
            view.parameter("charset").expect("valid parameters").expect("charset present"),
            CHARSET,
        )
}

#[metabench::benchmark(
    CONTENT_TYPE_PARAMETERS_HEADERS,
    "structured",
    "content_type_parameters_headers",
    gungraun_setup = content_type_many_map,
)]
#[bench::content_type_parameters()]
fn content_type_parameters_headers(map: &'static HeaderMap) -> usize {
    let content_type = TheirMapExt::typed_try_get::<headers::ContentType>(map)
        .expect("valid content type")
        .expect("present content type");
    let mime = headers::Mime::from(content_type);
    let mut total = 0;
    for (name, _parameter) in mime.params() {
        total += name.as_str().len();
    }
    let total = expect_usize(total, PARAMETER_NAME_BYTES)
        + expect_bytes(mime.get_param("charset").expect("charset present").as_str().as_bytes(), CHARSET);
    consume(mime);
    total
}

#[metabench::benchmark(
    CONTENT_TYPE_OWNED_HTTP_HEADERS,
    "structured",
    "content_type_owned_http_headers",
    gungraun_setup = json_map,
)]
#[bench::content_type_owned()]
fn content_type_owned_http_headers(map: &'static HeaderMap) -> usize {
    let content_type = ContentType::owned(map).expect("valid content type").expect("present content type");
    let mut total = expect_bytes(content_type.type_().expect("utf-8 type").as_bytes(), b"application");
    total += expect_bytes(content_type.subtype().expect("utf-8 subtype").as_bytes(), b"json");
    total += expect_bytes(
        content_type
            .parameter("charset")
            .expect("valid parameters")
            .expect("charset present"),
        CHARSET,
    );
    let total = expect_usize(total, 20);
    consume(content_type);
    total
}

#[metabench::benchmark(
    CONTENT_TYPE_OWNED_HEADERS,
    "structured",
    "content_type_owned_headers",
    gungraun_setup = json_map,
)]
#[bench::content_type_owned()]
fn content_type_owned_headers(map: &'static HeaderMap) -> usize {
    let content_type = TheirMapExt::typed_try_get::<headers::ContentType>(map)
        .expect("valid content type")
        .expect("present content type");
    let mime = headers::Mime::from(content_type);
    let total = inspect_mime(&mime);
    consume(mime);
    total
}

#[metabench::benchmark(
    CONTENT_TYPE_MALFORMED_HTTP_HEADERS,
    "structured",
    "content_type_malformed_http_headers",
    gungraun_setup = degraded_map,
)]
#[bench::content_type_malformed()]
fn content_type_malformed_http_headers(map: &'static HeaderMap) -> usize {
    let rejected = ContentType::view(map).is_err();
    expect_usize(usize::from(rejected), 1)
}

#[metabench::benchmark(
    CONTENT_TYPE_MALFORMED_HEADERS,
    "structured",
    "content_type_malformed_headers",
    gungraun_setup = degraded_map,
)]
#[bench::content_type_malformed()]
fn content_type_malformed_headers(map: &'static HeaderMap) -> usize {
    let rejected = TheirMapExt::typed_try_get::<headers::ContentType>(map).is_err();
    expect_usize(usize::from(rejected), 1)
}

// ── group: authorization ─────────────────────────────────────────────────────

fn basic_credentials() -> (&'static HeaderMap, BasicCredentials) {
    (&BASIC_REQUEST, BasicCredentials::new())
}

fn extract_basic<'a>(map: &HeaderMap, output: &'a mut BasicCredentials) -> &'a BasicCredentials {
    Authorization::<Basic>::view(map)
        .expect("valid credentials")
        .expect("present credentials")
        .extract(output)
        .expect("valid decoded credentials")
}

#[metabench::benchmark(
    BASIC_DECODE_HTTP_HEADERS,
    "authorization",
    "basic_decode_http_headers",
    gungraun_setup = basic_credentials,
    gungraun_teardown = drop_it,
)]
#[bench::basic_decode()]
fn basic_decode_http_headers(state: (&'static HeaderMap, BasicCredentials)) -> (usize, BasicCredentials) {
    let (map, mut output) = state;
    let total = {
        let credentials = extract_basic(map, &mut output);
        expect_bytes(credentials.username(), BASIC_USERNAME) + expect_bytes(credentials.password(), BASIC_PASSWORD)
    };
    (expect_usize(total, 17), output)
}

#[metabench::benchmark(
    BASIC_DECODE_HEADERS,
    "authorization",
    "basic_decode_headers",
    gungraun_setup = basic_credentials,
    gungraun_teardown = drop_it,
)]
#[bench::basic_decode()]
fn basic_decode_headers(state: (&'static HeaderMap, BasicCredentials)) -> (usize, BasicCredentials) {
    let (map, output) = state;
    let credentials = TheirMapExt::typed_try_get::<headers::Authorization<headers::authorization::Basic>>(map)
        .expect("valid credentials")
        .expect("present credentials");
    let total =
        expect_bytes(credentials.username().as_bytes(), BASIC_USERNAME) + expect_bytes(credentials.password().as_bytes(), BASIC_PASSWORD);
    let total = expect_usize(total, 17);
    consume(credentials);
    (total, output)
}

#[metabench::benchmark(
    BASIC_VALIDATE_HTTP_HEADERS,
    "authorization",
    "basic_validate_http_headers",
    gungraun_setup = basic_map,
)]
#[bench::basic_validate()]
fn basic_validate_http_headers(map: &'static HeaderMap) -> usize {
    let view = Authorization::<Basic>::view(map)
        .expect("valid credentials")
        .expect("present credentials");
    expect_bytes(view.credentials(), BASIC_ENCODED)
}

#[metabench::benchmark(
    BASIC_VALIDATE_HEADERS,
    "authorization",
    "basic_validate_headers",
    gungraun_setup = basic_map,
)]
#[bench::basic_validate()]
fn basic_validate_headers(map: &'static HeaderMap) -> usize {
    let credentials = TheirMapExt::typed_try_get::<headers::Authorization<headers::authorization::Basic>>(map)
        .expect("valid credentials")
        .expect("present credentials");
    let total = expect_usize(credentials.username().len() + credentials.password().len(), 17);
    consume(credentials);
    total
}

#[metabench::benchmark(
    BEARER_BORROWED_HTTP_HEADERS,
    "authorization",
    "bearer_borrowed_http_headers",
    gungraun_setup = bearer_map,
)]
#[bench::bearer_borrowed()]
fn bearer_borrowed_http_headers(map: &'static HeaderMap) -> usize {
    let view = Authorization::<Bearer>::view(map)
        .expect("valid credentials")
        .expect("present credentials");
    expect_bytes(view.token(), BEARER_TOKEN)
}

#[metabench::benchmark(
    BEARER_BORROWED_HEADERS,
    "authorization",
    "bearer_borrowed_headers",
    gungraun_setup = bearer_map,
)]
#[bench::bearer_borrowed()]
fn bearer_borrowed_headers(map: &'static HeaderMap) -> usize {
    let credentials = TheirMapExt::typed_try_get::<headers::Authorization<headers::authorization::Bearer>>(map)
        .expect("valid credentials")
        .expect("present credentials");
    let total = expect_bytes(credentials.token().as_bytes(), BEARER_TOKEN);
    consume(credentials);
    total
}

#[metabench::benchmark(
    BASIC_ENCODE_HTTP_HEADERS,
    "authorization",
    "basic_encode_http_headers",
    gungraun_setup = empty_map,
    gungraun_teardown = drop_it,
)]
#[bench::basic_encode()]
fn basic_encode_http_headers(mut map: HeaderMap) -> (usize, HeaderMap) {
    let credentials = AuthorizationOwned::<Basic>::basic(BASIC_USERNAME, BASIC_PASSWORD).expect("legal credentials");
    expect_inserted(Authorization::<Basic>::insert(&mut map, credentials));
    let stored = map.get(&AUTHORIZATION).expect("inserted credentials");
    let length = expect_bytes(stored.as_bytes(), fixtures::BASIC_VALUE);
    (length, map)
}

#[metabench::benchmark(
    BASIC_ENCODE_HEADERS,
    "authorization",
    "basic_encode_headers",
    gungraun_setup = empty_map,
    gungraun_teardown = drop_it,
)]
#[bench::basic_encode()]
fn basic_encode_headers(mut map: HeaderMap) -> (usize, HeaderMap) {
    let credentials = headers::Authorization::basic("aladdin", "opensesame");
    TheirMapExt::typed_insert(&mut map, credentials);
    let stored = map.get(&AUTHORIZATION).expect("inserted credentials");
    let length = expect_bytes(stored.as_bytes(), fixtures::BASIC_VALUE);
    (length, map)
}

// ── group: insertion ─────────────────────────────────────────────────────────

fn our_user_agent() -> (HeaderMap, UserAgentOwned) {
    (
        empty_map(),
        UserAgentOwned::try_from(field_value(USER_AGENT_VALUE)).expect("legal user agent"),
    )
}

fn their_user_agent() -> (HeaderMap, headers::UserAgent) {
    let text = str::from_utf8(USER_AGENT_VALUE).expect("utf-8 corpus");
    (empty_map(), headers::UserAgent::from_str(text).expect("legal user agent"))
}

#[metabench::benchmark(
    INSERT_USER_AGENT_HTTP_HEADERS,
    "insertion",
    "insert_user_agent_http_headers",
    gungraun_setup = our_user_agent,
    gungraun_teardown = drop_it,
)]
#[bench::insert_user_agent()]
fn insert_user_agent_http_headers(state: (HeaderMap, UserAgentOwned)) -> (usize, HeaderMap) {
    let (mut map, agent) = state;
    expect_inserted(UserAgent::insert(&mut map, agent));
    let stored = map.get(&USER_AGENT).expect("inserted user agent");
    let length = expect_bytes(stored.as_bytes(), USER_AGENT_VALUE);
    (length, map)
}

#[metabench::benchmark(
    INSERT_USER_AGENT_HEADERS,
    "insertion",
    "insert_user_agent_headers",
    gungraun_setup = their_user_agent,
    gungraun_teardown = drop_it,
)]
#[bench::insert_user_agent()]
fn insert_user_agent_headers(state: (HeaderMap, headers::UserAgent)) -> (usize, HeaderMap) {
    let (mut map, agent) = state;
    TheirMapExt::typed_insert(&mut map, agent);
    let stored = map.get(&USER_AGENT).expect("inserted user agent");
    let length = expect_bytes(stored.as_bytes(), USER_AGENT_VALUE);
    (length, map)
}

fn our_content_type() -> (HeaderMap, ContentTypeOwned) {
    (empty_map(), ContentTypeOwned::json())
}

fn their_content_type() -> (HeaderMap, headers::ContentType) {
    (empty_map(), headers::ContentType::json())
}

#[metabench::benchmark(
    INSERT_CONTENT_TYPE_HTTP_HEADERS,
    "insertion",
    "insert_content_type_http_headers",
    gungraun_setup = our_content_type,
    gungraun_teardown = drop_it,
)]
#[bench::insert_content_type()]
fn insert_content_type_http_headers(state: (HeaderMap, ContentTypeOwned)) -> (usize, HeaderMap) {
    let (mut map, content_type) = state;
    expect_inserted(ContentType::insert(&mut map, content_type));
    let stored = map.get(&CONTENT_TYPE).expect("inserted content type");
    let length = expect_bytes(stored.as_bytes(), b"application/json");
    (length, map)
}

#[metabench::benchmark(
    INSERT_CONTENT_TYPE_HEADERS,
    "insertion",
    "insert_content_type_headers",
    gungraun_setup = their_content_type,
    gungraun_teardown = drop_it,
)]
#[bench::insert_content_type()]
fn insert_content_type_headers(state: (HeaderMap, headers::ContentType)) -> (usize, HeaderMap) {
    let (mut map, content_type) = state;
    TheirMapExt::typed_insert(&mut map, content_type);
    let stored = map.get(&CONTENT_TYPE).expect("inserted content type");
    let length = expect_bytes(stored.as_bytes(), b"application/json");
    (length, map)
}

fn our_cache_control() -> (HeaderMap, CacheControlOwned) {
    let control = CacheControlOwned::builder()
        .max_age(MAX_AGE)
        .private()
        .build()
        .expect("legal directives");
    (empty_map(), control)
}

fn their_cache_control() -> (HeaderMap, headers::CacheControl) {
    let control = headers::CacheControl::new().with_max_age(MAX_AGE).with_private();
    (empty_map(), control)
}

#[metabench::benchmark(
    INSERT_CACHE_CONTROL_HTTP_HEADERS,
    "insertion",
    "insert_cache_control_http_headers",
    gungraun_setup = our_cache_control,
    gungraun_teardown = drop_it,
)]
#[bench::insert_cache_control()]
fn insert_cache_control_http_headers(state: (HeaderMap, CacheControlOwned)) -> (usize, HeaderMap) {
    let (mut map, control) = state;
    expect_inserted(CacheControl::insert(&mut map, control));
    let stored = map.get(&CACHE_CONTROL).expect("inserted cache control");
    let length = expect_bytes(stored.as_bytes(), b"max-age=3600, private");
    (length, map)
}

#[metabench::benchmark(
    INSERT_CACHE_CONTROL_HEADERS,
    "insertion",
    "insert_cache_control_headers",
    gungraun_setup = their_cache_control,
    gungraun_teardown = drop_it,
)]
#[bench::insert_cache_control()]
fn insert_cache_control_headers(state: (HeaderMap, headers::CacheControl)) -> (usize, HeaderMap) {
    let (mut map, control) = state;
    TheirMapExt::typed_insert(&mut map, control);
    let stored = map.get(&CACHE_CONTROL).expect("inserted cache control");
    let length = expect_bytes(stored.as_bytes(), b"private, max-age=3600");
    (length, map)
}

// ── group: ownership ─────────────────────────────────────────────────────────

fn our_cookies() -> (HeaderMap, SetCookieOwned) {
    let mut cookies = SetCookieOwned::new();
    for cookie in SET_COOKIE_VALUES {
        cookies.push(field_value(cookie)).expect("legal cookie");
    }
    (empty_map(), cookies)
}

#[metabench::benchmark(
    OWNERSHIP_USER_AGENT_MOVED,
    "ownership",
    "ownership_user_agent_moved",
    gungraun_setup = our_user_agent,
    gungraun_teardown = drop_it,
)]
#[bench::ownership_user_agent()]
fn ownership_user_agent_moved(state: (HeaderMap, UserAgentOwned)) -> (usize, HeaderMap) {
    let (mut map, agent) = state;
    expect_inserted(UserAgent::insert(&mut map, agent));
    let stored = map.get(&USER_AGENT).expect("inserted user agent");
    let length = expect_bytes(stored.as_bytes(), USER_AGENT_VALUE);
    (length, map)
}

#[metabench::benchmark(
    OWNERSHIP_USER_AGENT_CLONED,
    "ownership",
    "ownership_user_agent_cloned",
    gungraun_setup = our_user_agent,
    gungraun_teardown = drop_it,
)]
#[bench::ownership_user_agent()]
fn ownership_user_agent_cloned(state: (HeaderMap, UserAgentOwned)) -> (usize, HeaderMap, UserAgentOwned) {
    let (mut map, agent) = state;
    expect_inserted(UserAgent::insert(&mut map, agent.clone()));
    let stored = map.get(&USER_AGENT).expect("inserted user agent");
    let length = expect_bytes(stored.as_bytes(), USER_AGENT_VALUE);
    (length, map, agent)
}

#[metabench::benchmark(
    OWNERSHIP_SET_COOKIE_MOVED,
    "ownership",
    "ownership_set_cookie_moved",
    gungraun_setup = our_cookies,
    gungraun_teardown = drop_it,
)]
#[bench::ownership_set_cookie()]
fn ownership_set_cookie_moved(state: (HeaderMap, SetCookieOwned)) -> (usize, HeaderMap) {
    let (mut map, cookies) = state;
    expect_inserted(SetCookie::insert(&mut map, cookies));
    let stored = map.get_all(&SET_COOKIE).iter().count();
    (expect_usize(stored, 4), map)
}

#[metabench::benchmark(
    OWNERSHIP_SET_COOKIE_CLONED,
    "ownership",
    "ownership_set_cookie_cloned",
    gungraun_setup = our_cookies,
    gungraun_teardown = drop_it,
)]
#[bench::ownership_set_cookie()]
fn ownership_set_cookie_cloned(state: (HeaderMap, SetCookieOwned)) -> (usize, HeaderMap, SetCookieOwned) {
    let (mut map, cookies) = state;
    expect_inserted(SetCookie::insert(&mut map, cookies.clone()));
    let stored = map.get_all(&SET_COOKIE).iter().count();
    (expect_usize(stored, 4), map, cookies)
}

// ── group: credentials ───────────────────────────────────────────────────────

fn warm_credentials() -> (&'static HeaderMap, BasicCredentials) {
    let map: &'static HeaderMap = &BASIC_REQUEST;
    let mut output = BasicCredentials::new();
    assert!(!extract_basic(map, &mut output).username().is_empty(), "credentials must decode");
    (map, output)
}

fn large_warm_credentials() -> (&'static HeaderMap, BasicCredentials) {
    let map: &'static HeaderMap = &LARGE_BASIC;
    let mut output = BasicCredentials::new();
    assert!(!extract_basic(map, &mut output).password().is_empty(), "credentials must decode");
    (map, output)
}

fn high_water_warm_credentials() -> (&'static HeaderMap, BasicCredentials) {
    let mut output = BasicCredentials::new();
    assert!(
        !extract_basic(&LARGE_BASIC, &mut output).password().is_empty(),
        "credentials must decode"
    );
    (&BASIC_REQUEST, output)
}

fn decode_rounds(map: &HeaderMap, output: &mut BasicCredentials) -> usize {
    let mut total = 0;
    for _ in 0..CREDENTIAL_ROUNDS {
        let credentials = extract_basic(map, output);
        total += credentials.username().len() + credentials.password().len();
    }
    total
}

fn decode_rounds_fresh(map: &HeaderMap) -> usize {
    let mut total = 0;
    for _ in 0..CREDENTIAL_ROUNDS {
        let mut output = BasicCredentials::new();
        let credentials = extract_basic(map, &mut output);
        total += credentials.username().len() + credentials.password().len();
    }
    total
}

#[metabench::benchmark(
    CREDENTIALS_BASIC_REUSED,
    "credentials",
    "credentials_basic_reused",
    gungraun_setup = warm_credentials,
    gungraun_teardown = drop_it,
)]
#[bench::credentials_basic()]
fn credentials_basic_reused(state: (&'static HeaderMap, BasicCredentials)) -> (usize, BasicCredentials) {
    let (map, mut output) = state;
    let total = decode_rounds(map, &mut output);
    (expect_usize(total, 17 * CREDENTIAL_ROUNDS), output)
}

#[metabench::benchmark(
    CREDENTIALS_BASIC_HIGH_WATER_REUSED,
    "credentials",
    "credentials_basic_high_water_reused",
    gungraun_setup = high_water_warm_credentials,
    gungraun_teardown = drop_it,
)]
#[bench::credentials_basic_high_water()]
fn credentials_basic_high_water_reused(state: (&'static HeaderMap, BasicCredentials)) -> (usize, BasicCredentials) {
    let (map, mut output) = state;
    let total = decode_rounds(map, &mut output);
    (expect_usize(total, 17 * CREDENTIAL_ROUNDS), output)
}

#[metabench::benchmark(
    CREDENTIALS_BASIC_FRESH,
    "credentials",
    "credentials_basic_fresh",
    gungraun_setup = basic_map,
)]
#[bench::credentials_basic()]
fn credentials_basic_fresh(map: &'static HeaderMap) -> usize {
    expect_usize(decode_rounds_fresh(map), 17 * CREDENTIAL_ROUNDS)
}

#[metabench::benchmark(
    CREDENTIALS_LARGE_REUSED,
    "credentials",
    "credentials_large_reused",
    gungraun_setup = large_warm_credentials,
    gungraun_teardown = drop_it,
)]
#[bench::credentials_large()]
fn credentials_large_reused(state: (&'static HeaderMap, BasicCredentials)) -> (usize, BasicCredentials) {
    let (map, mut output) = state;
    let total = decode_rounds(map, &mut output);
    (expect_usize(total, 2999 * CREDENTIAL_ROUNDS), output)
}

#[metabench::benchmark(
    CREDENTIALS_LARGE_FRESH,
    "credentials",
    "credentials_large_fresh",
    gungraun_setup = large_map,
)]
#[bench::credentials_large()]
fn credentials_large_fresh(map: &'static HeaderMap) -> usize {
    expect_usize(decode_rounds_fresh(map), 2999 * CREDENTIAL_ROUNDS)
}

// ── group: scanning ──────────────────────────────────────────────────────────

/// Historical scalar code retained for one before-and-after comparison.
///
/// Ordinary scalar comparisons call the production scalar backend directly.
/// `is_token68_legacy` is different in kind: it is the per-byte `matches!`
/// chain the `Authorization` parser used *before* `http_headers_simd::is_token68` existed.
/// It is kept so the report can price what that change bought, and it is not
/// what any production path runs today.
mod reference {
    /// The per-byte `matches!` chain the `Authorization` parser ran before
    /// `http_headers_simd::is_token68` existed. Retained only as a before-and-after
    /// reference; no production path executes this today.
    pub(super) fn is_token68_legacy(bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let data_end = bytes.iter().position(|byte| *byte == b'=').unwrap_or(bytes.len());
        data_end > 0
            && bytes[..data_end].iter().all(|byte| {
                matches!(
                    byte,
                    b'A'..=b'Z'
                        | b'a'..=b'z'
                        | b'0'..=b'9'
                        | b'-'
                        | b'.'
                        | b'_'
                        | b'~'
                        | b'+'
                        | b'/'
                )
            })
            && bytes[data_end..].iter().all(|byte| *byte == b'=')
    }
}

fn token_of(length: usize) -> Vec<u8> {
    let mut bytes = vec![b'a'; length];
    if let Some(last) = bytes.last_mut() {
        *last = b'z';
    }
    bytes
}

fn token_15() -> Vec<u8> {
    token_of(15)
}

fn token_16() -> Vec<u8> {
    token_of(16)
}

fn token_31() -> Vec<u8> {
    token_of(31)
}

fn token_32() -> Vec<u8> {
    token_of(32)
}

fn token_33() -> Vec<u8> {
    token_of(33)
}

fn token_512() -> Vec<u8> {
    token_of(512)
}

fn field_value_bytes() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(512);
    while bytes.len() < 512 {
        bytes.extend_from_slice(b"text/html; charset=utf-8, application/json; q=0.9, ");
    }
    bytes.truncate(512);
    bytes
}

/// 512 bytes holding one delimiter every 32, so a scan finds work everywhere.
fn single_line_list() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(512);
    while bytes.len() < 512 {
        bytes.extend_from_slice(b"directiveabcdefghijklmnop=value,");
    }
    bytes.truncate(512);
    bytes
}

/// The same 512 bytes split into eight short field lines.
fn multi_line_list() -> Vec<Vec<u8>> {
    let line = single_line_list();
    (0..8).map(|index| line[index * 64..][..64].to_vec()).collect()
}

fn comma_items_with_irrelevant_bytes() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(512);
    while bytes.len() < 512 {
        bytes.extend_from_slice(b" alpha=bravo ; charlie = delta , ");
    }
    bytes.truncate(510);
    bytes.extend_from_slice(b",x");
    bytes
}

fn scan_all(bytes: &[u8], find: fn(&[u8]) -> Option<usize>) -> usize {
    let mut position = 0;
    let mut found = 0;
    while position < bytes.len() {
        let Some(offset) = find(&bytes[position..]) else {
            break;
        };
        position += offset + 1;
        found += 1;
    }
    found
}

fn token_answer(ok: bool, bytes: &[u8], length: usize) -> usize {
    expect_usize(usize::from(ok) + bytes.len(), 1 + length)
}

#[metabench::benchmark(
    TOKEN_15_DISPATCHED,
    "scanning",
    "token_15_dispatched",
    gungraun_setup = token_15,
    gungraun_teardown = drop_it,
)]
#[bench::token_15()]
fn token_15_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token(&bytes), &bytes, 15);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_15_REFERENCE,
    "scanning",
    "token_15_reference",
    gungraun_setup = token_15,
    gungraun_teardown = drop_it,
)]
#[bench::token_15()]
fn token_15_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::benchmarking::is_token_scalar(&bytes), &bytes, 15);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_16_DISPATCHED,
    "scanning",
    "token_16_dispatched",
    gungraun_setup = token_16,
    gungraun_teardown = drop_it,
)]
#[bench::token_16()]
fn token_16_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token(&bytes), &bytes, 16);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_16_REFERENCE,
    "scanning",
    "token_16_reference",
    gungraun_setup = token_16,
    gungraun_teardown = drop_it,
)]
#[bench::token_16()]
fn token_16_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::benchmarking::is_token_scalar(&bytes), &bytes, 16);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_31_DISPATCHED,
    "scanning",
    "token_31_dispatched",
    gungraun_setup = token_31,
    gungraun_teardown = drop_it,
)]
#[bench::token_31()]
fn token_31_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token(&bytes), &bytes, 31);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_31_REFERENCE,
    "scanning",
    "token_31_reference",
    gungraun_setup = token_31,
    gungraun_teardown = drop_it,
)]
#[bench::token_31()]
fn token_31_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::benchmarking::is_token_scalar(&bytes), &bytes, 31);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_32_DISPATCHED,
    "scanning",
    "token_32_dispatched",
    gungraun_setup = token_32,
    gungraun_teardown = drop_it,
)]
#[bench::token_32()]
fn token_32_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token(&bytes), &bytes, 32);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_32_REFERENCE,
    "scanning",
    "token_32_reference",
    gungraun_setup = token_32,
    gungraun_teardown = drop_it,
)]
#[bench::token_32()]
fn token_32_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::benchmarking::is_token_scalar(&bytes), &bytes, 32);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_33_DISPATCHED,
    "scanning",
    "token_33_dispatched",
    gungraun_setup = token_33,
    gungraun_teardown = drop_it,
)]
#[bench::token_33()]
fn token_33_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token(&bytes), &bytes, 33);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_33_REFERENCE,
    "scanning",
    "token_33_reference",
    gungraun_setup = token_33,
    gungraun_teardown = drop_it,
)]
#[bench::token_33()]
fn token_33_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::benchmarking::is_token_scalar(&bytes), &bytes, 33);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_512_DISPATCHED,
    "scanning",
    "token_512_dispatched",
    gungraun_setup = token_512,
    gungraun_teardown = drop_it,
)]
#[bench::token_512()]
fn token_512_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token(&bytes), &bytes, 512);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN_512_REFERENCE,
    "scanning",
    "token_512_reference",
    gungraun_setup = token_512,
    gungraun_teardown = drop_it,
)]
#[bench::token_512()]
fn token_512_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::benchmarking::is_token_scalar(&bytes), &bytes, 512);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN68_150_DISPATCHED,
    "scanning",
    "token68_150_dispatched",
    gungraun_setup = bearer_token_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::token68_150()]
fn token68_150_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token68(&bytes), &bytes, BEARER_TOKEN.len());
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN68_150_REFERENCE,
    "scanning",
    "token68_150_reference",
    gungraun_setup = bearer_token_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::token68_150()]
fn token68_150_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(
        http_headers_simd::benchmarking::is_token68_scalar(&bytes),
        &bytes,
        BEARER_TOKEN.len(),
    );
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN68_512_DISPATCHED,
    "scanning",
    "token68_512_dispatched",
    gungraun_setup = token68_512_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::token68_512()]
fn token68_512_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token68(&bytes), &bytes, 512);
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN68_512_REFERENCE,
    "scanning",
    "token68_512_reference",
    gungraun_setup = token68_512_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::token68_512()]
fn token68_512_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::benchmarking::is_token68_scalar(&bytes), &bytes, 512);
    (answer, bytes)
}

// The same 150 bytes as `token68_150`, through a different byte-class kernel:
// identical input and harness, so the two rows differ only by the kernel.
#[metabench::benchmark(
    FIELD_VALUE_150_DISPATCHED,
    "scanning",
    "field_value_150_dispatched",
    gungraun_setup = bearer_token_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::field_value_150()]
fn field_value_150_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = expect_usize(usize::from(http_headers_simd::is_field_value(&bytes)), 1);
    (answer, bytes)
}

#[metabench::benchmark(
    FIELD_VALUE_150_REFERENCE,
    "scanning",
    "field_value_150_reference",
    gungraun_setup = bearer_token_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::field_value_150()]
fn field_value_150_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = expect_usize(usize::from(http_headers_simd::benchmarking::is_field_value_scalar(&bytes)), 1);
    (answer, bytes)
}

#[metabench::benchmark(
    FIELD_VALUE_512_DISPATCHED,
    "scanning",
    "field_value_512_dispatched",
    gungraun_setup = field_value_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::field_value_512()]
fn field_value_512_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = expect_usize(usize::from(http_headers_simd::is_field_value(&bytes)), 1);
    (answer, bytes)
}

#[metabench::benchmark(
    FIELD_VALUE_512_REFERENCE,
    "scanning",
    "field_value_512_reference",
    gungraun_setup = field_value_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::field_value_512()]
fn field_value_512_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = expect_usize(usize::from(http_headers_simd::benchmarking::is_field_value_scalar(&bytes)), 1);
    (answer, bytes)
}

#[metabench::benchmark(
    DELIMITERS_SINGLE_LINE_DISPATCHED,
    "scanning",
    "delimiters_single_line_dispatched",
    gungraun_setup = single_line_list,
    gungraun_teardown = drop_it,
)]
#[bench::delimiters_single_line()]
fn delimiters_single_line_dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let found = expect_usize(scan_all(&bytes, http_headers_simd::find_interesting), 16);
    (found, bytes)
}

#[metabench::benchmark(
    DELIMITERS_SINGLE_LINE_REFERENCE,
    "scanning",
    "delimiters_single_line_reference",
    gungraun_setup = single_line_list,
    gungraun_teardown = drop_it,
)]
#[bench::delimiters_single_line()]
fn delimiters_single_line_reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let found = expect_usize(scan_all(&bytes, http_headers_simd::benchmarking::find_interesting_scalar), 16);
    (found, bytes)
}

#[metabench::benchmark(
    DELIMITERS_MULTI_LINE_DISPATCHED,
    "scanning",
    "delimiters_multi_line_dispatched",
    gungraun_setup = multi_line_list,
    gungraun_teardown = drop_it,
)]
#[bench::delimiters_multi_line()]
fn delimiters_multi_line_dispatched(lines: Vec<Vec<u8>>) -> (usize, Vec<Vec<u8>>) {
    let mut found = 0;
    for line in &lines {
        found += scan_all(line, http_headers_simd::find_interesting);
    }
    (expect_usize(found, 16), lines)
}

#[metabench::benchmark(
    DELIMITERS_MULTI_LINE_REFERENCE,
    "scanning",
    "delimiters_multi_line_reference",
    gungraun_setup = multi_line_list,
    gungraun_teardown = drop_it,
)]
#[bench::delimiters_multi_line()]
fn delimiters_multi_line_reference(lines: Vec<Vec<u8>>) -> (usize, Vec<Vec<u8>>) {
    let mut found = 0;
    for line in &lines {
        found += scan_all(line, http_headers_simd::benchmarking::find_interesting_scalar);
    }
    (expect_usize(found, 16), lines)
}

#[metabench::benchmark(
    COMMA_ITEMS_IRRELEVANT_BYTES,
    "scanning",
    "comma_items_irrelevant_bytes",
    gungraun_setup = comma_items_with_irrelevant_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::comma_items_irrelevant_bytes()]
fn comma_items_irrelevant_bytes(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let lines = FieldLines::single(&http_headers::FieldName::Vary, &bytes);
    let total = lines.comma_items().map(|item| item.expect("valid item").len()).sum::<usize>();
    (black_box(total), bytes)
}

// ── group: list_scanning ─────────────────────────────────────────────────────
//
// Cases straddling the two list-scan thresholds that `scanning` above does
// not cover: `LIST_SIMD_THRESHOLD` (`WIDTH`, 16 bytes — also
// `cors::LIST_SCAN_MIN_LEN`) and `LIST_SHORT_LIMIT` (32 bytes).

/// A comma-separated list of single-byte tokens totalling exactly `length`
/// bytes, e.g. `a,a,a` — long enough to exercise `scan_token_list` at a
/// chosen length without ever producing a trailing empty member.
fn token_list_of(length: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        if !bytes.is_empty() {
            bytes.push(b',');
        }
        bytes.push(b'a');
    }
    bytes.truncate(length);
    if bytes.last() == Some(&b',')
        && let Some(last) = bytes.last_mut()
    {
        *last = b'a';
    }
    bytes
}

fn list_scan_15() -> Vec<u8> {
    token_list_of(15)
}

fn list_scan_16() -> Vec<u8> {
    token_list_of(16)
}

fn list_scan_17() -> Vec<u8> {
    token_list_of(17)
}

fn list_scan_31() -> Vec<u8> {
    token_list_of(31)
}

fn list_scan_32() -> Vec<u8> {
    token_list_of(32)
}

fn list_scan_33() -> Vec<u8> {
    token_list_of(33)
}

fn list_scan_answer(scan: http_headers_simd::TokenListScan, length: usize) -> usize {
    let ok = matches!(scan, http_headers_simd::TokenListScan::Members);
    expect_usize(usize::from(ok) + length, 1 + length)
}

macro_rules! list_scan_case {
    (
        $dispatched:ident,
        $dispatched_name:literal,
        $reference:ident,
        $reference_name:literal,
        $bench:ident,
        $setup:ident
    ) => {
        paste::paste! {
            #[metabench::benchmark(
                [<$dispatched:upper>],
                "list_scanning",
                $dispatched_name,
                gungraun_setup = $setup,
                gungraun_teardown = drop_it,
            )]
            #[bench::$bench()]
            fn $dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
                let scan = http_headers_simd::scan_token_list(
                    &bytes,
                    http_headers_simd::EmptyMembers::Skip,
                );
                let answer = list_scan_answer(scan, bytes.len());
                (answer, bytes)
            }

            #[metabench::benchmark(
                [<$reference:upper>],
                "list_scanning",
                $reference_name,
                gungraun_setup = $setup,
                gungraun_teardown = drop_it,
            )]
            #[bench::$bench()]
            fn $reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
                let scan = http_headers_simd::benchmarking::scan_token_list_scalar(
                    &bytes,
                    http_headers_simd::EmptyMembers::Skip,
                );
                let answer = list_scan_answer(scan, bytes.len());
                (answer, bytes)
            }
        }
    };
}

list_scan_case!(
    list_scan_15_dispatched,
    "list_scan_15_dispatched",
    list_scan_15_reference,
    "list_scan_15_reference",
    list_scan_15,
    list_scan_15
);
list_scan_case!(
    list_scan_16_dispatched,
    "list_scan_16_dispatched",
    list_scan_16_reference,
    "list_scan_16_reference",
    list_scan_16,
    list_scan_16
);
list_scan_case!(
    list_scan_17_dispatched,
    "list_scan_17_dispatched",
    list_scan_17_reference,
    "list_scan_17_reference",
    list_scan_17,
    list_scan_17
);
list_scan_case!(
    list_scan_31_dispatched,
    "list_scan_31_dispatched",
    list_scan_31_reference,
    "list_scan_31_reference",
    list_scan_31,
    list_scan_31
);
list_scan_case!(
    list_scan_32_dispatched,
    "list_scan_32_dispatched",
    list_scan_32_reference,
    "list_scan_32_reference",
    list_scan_32,
    list_scan_32
);
list_scan_case!(
    list_scan_33_dispatched,
    "list_scan_33_dispatched",
    list_scan_33_reference,
    "list_scan_33_reference",
    list_scan_33,
    list_scan_33
);

// ── group: uri_scanning ──────────────────────────────────────────────────────
//
// Cases straddling `URI_SIMD_THRESHOLD` (16 bytes).

/// A `path-absolute` URI reference of exactly `length` bytes.
fn uri_path_of(length: usize) -> Vec<u8> {
    let mut bytes = vec![b'a'; length.max(1)];
    bytes[0] = b'/';
    bytes
}

fn uri_path_15() -> Vec<u8> {
    uri_path_of(15)
}

fn uri_path_16() -> Vec<u8> {
    uri_path_of(16)
}

fn uri_path_17() -> Vec<u8> {
    uri_path_of(17)
}

fn uri_path_answer(ok: bool, length: usize) -> usize {
    expect_usize(usize::from(ok) + length, 1 + length)
}

macro_rules! uri_path_case {
    (
        $dispatched:ident,
        $dispatched_name:literal,
        $reference:ident,
        $reference_name:literal,
        $bench:ident,
        $setup:ident
    ) => {
        paste::paste! {
            #[metabench::benchmark(
                [<$dispatched:upper>],
                "uri_scanning",
                $dispatched_name,
                gungraun_setup = $setup,
                gungraun_teardown = drop_it,
            )]
            #[bench::$bench()]
            fn $dispatched(bytes: Vec<u8>) -> (usize, Vec<u8>) {
                let answer =
                    uri_path_answer(http_headers_simd::is_simple_uri_path(&bytes), bytes.len());
                (answer, bytes)
            }

            #[metabench::benchmark(
                [<$reference:upper>],
                "uri_scanning",
                $reference_name,
                gungraun_setup = $setup,
                gungraun_teardown = drop_it,
            )]
            #[bench::$bench()]
            fn $reference(bytes: Vec<u8>) -> (usize, Vec<u8>) {
                let answer = uri_path_answer(
                    http_headers_simd::benchmarking::is_simple_uri_path_scalar(&bytes),
                    bytes.len(),
                );
                (answer, bytes)
            }
        }
    };
}

uri_path_case!(
    uri_path_15_dispatched,
    "uri_path_15_dispatched",
    uri_path_15_reference,
    "uri_path_15_reference",
    uri_path_15,
    uri_path_15
);
uri_path_case!(
    uri_path_16_dispatched,
    "uri_path_16_dispatched",
    uri_path_16_reference,
    "uri_path_16_reference",
    uri_path_16,
    uri_path_16
);
uri_path_case!(
    uri_path_17_dispatched,
    "uri_path_17_dispatched",
    uri_path_17_reference,
    "uri_path_17_reference",
    uri_path_17,
    uri_path_17
);

// ── group: forced_uri_backends ──────────────────────────────────────────────
//
// The same input is sent directly to every compiled URI backend. Unsupported
// backends return `None`, so the x86 and AArch64 scheduled runners together
// produce a real count for SSE2, SSSE3, SSE4.2, and NEON without relying on
// the dispatcher's choice for the host.

fn uri_path_64() -> Vec<u8> {
    uri_path_of(64)
}

fn forced_uri_answer(result: Option<bool>, length: usize) -> usize {
    black_box(usize::from(result.unwrap_or(false)) + length)
}

macro_rules! forced_uri_backend_case {
    ($name:ident, $benchmark_name:literal, $scanner:path) => {
        paste::paste! {
            #[metabench::benchmark(
                [<$name:upper>],
                "forced_uri_backends",
                $benchmark_name,
                gungraun_setup = uri_path_64,
                gungraun_teardown = drop_it,
            )]
            #[bench::uri_path_64()]
            fn $name(bytes: Vec<u8>) -> (usize, Vec<u8>) {
                let answer = forced_uri_answer($scanner(&bytes), bytes.len());
                (answer, bytes)
            }
        }
    };
}

forced_uri_backend_case!(
    uri_path_64_sse2,
    "uri_path_64_sse2",
    http_headers_simd::benchmarking::is_simple_uri_path_sse2
);
forced_uri_backend_case!(
    uri_path_64_ssse3,
    "uri_path_64_ssse3",
    http_headers_simd::benchmarking::is_simple_uri_path_ssse3
);
forced_uri_backend_case!(
    uri_path_64_sse42,
    "uri_path_64_sse42",
    http_headers_simd::benchmarking::is_simple_uri_path_sse42
);
forced_uri_backend_case!(
    uri_path_64_neon,
    "uri_path_64_neon",
    http_headers_simd::benchmarking::is_simple_uri_path_neon
);

// ── group: credential_scan ───────────────────────────────────────────────────
//
// Measures the shared `token68` scanner on the credential in the corpus.

fn bearer_token_bytes() -> Vec<u8> {
    let bytes = BEARER_TOKEN.to_vec();
    black_box(http_headers_simd::is_token68(&bytes));
    bytes
}

fn token68_512_bytes() -> Vec<u8> {
    let bytes = token_512();
    black_box(http_headers_simd::is_token68(&bytes));
    bytes
}

#[metabench::benchmark(
    TOKEN68_CREDENTIAL_SHARED_SCANNER,
    "credential_scan",
    "token68_credential_shared_scanner",
    gungraun_setup = bearer_token_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::token68_credential()]
fn token68_credential_shared_scanner(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(http_headers_simd::is_token68(&bytes), &bytes, BEARER_TOKEN.len());
    (answer, bytes)
}

#[metabench::benchmark(
    TOKEN68_CREDENTIAL_MATCH_CHAIN,
    "credential_scan",
    "token68_credential_match_chain",
    gungraun_setup = bearer_token_bytes,
    gungraun_teardown = drop_it,
)]
#[bench::token68_credential()]
fn token68_credential_match_chain(bytes: Vec<u8>) -> (usize, Vec<u8>) {
    let answer = token_answer(reference::is_token68_legacy(&bytes), &bytes, BEARER_TOKEN.len());
    (answer, bytes)
}

// ── group: custom ────────────────────────────────────────────────────────────

/// A downstream header written with nothing but the public extension API.
///
/// It names `User-Agent` and validates exactly what the built-in `UserAgent`
/// validates, so the pair prices framework overhead rather than two different
/// grammars over two different values.
struct DownstreamAgent(FieldValue);

/// The borrowed view of [`DownstreamAgent`].
#[derive(Clone, Copy)]
struct DownstreamAgentView<'a>(FieldValueRef<'a>);

impl<'a> DownstreamAgentView<'a> {
    fn as_bytes(self) -> &'a [u8] {
        self.0.as_bytes()
    }
}

impl Field for DownstreamAgent {
    type View<'a> = DownstreamAgentView<'a>;
    type Owned = Self;

    fn name() -> &'static http_headers::FieldName {
        &http_headers::FieldName::UserAgent
    }

    fn view_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(values) = source.lines(Self::name()) else {
            return Ok(None);
        };
        let value = values.exactly_one()?;
        if value.as_bytes().is_empty() || !http_headers_simd::is_field_value(value.as_bytes()) {
            return Err(DecodeError::new(
                &http_headers::FieldName::UserAgent,
                DecodeErrorKind::InvalidSyntax,
            ));
        }
        Ok(Some(DownstreamAgentView(value)))
    }

    fn owned_with<S>(source: &S, mode: DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        Self::view_with(source, mode).map(|view| {
            view.map(|view| {
                Self(
                    view.0
                        .try_to_field_value()
                        .expect("view_with validated the HTTP field-value grammar"),
                )
            })
        })
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), EncodedValues::single(value.0))
    }
}

fn downstream_agent() -> (HeaderMap, DownstreamAgent) {
    (empty_map(), DownstreamAgent(field_value(USER_AGENT_VALUE)))
}

struct RawUserAgentSource;

impl FieldSource for RawUserAgentSource {
    fn lines(&self, name: &'static http_headers::FieldName) -> Option<FieldLines<'_>> {
        (name == &http_headers::FieldName::UserAgent).then(|| FieldLines::single(name, USER_AGENT_VALUE))
    }
}

fn raw_user_agent_source() -> RawUserAgentSource {
    RawUserAgentSource
}

#[metabench::benchmark(
    CUSTOM_BORROWED_BUILTIN,
    "custom",
    "custom_borrowed_builtin",
    gungraun_setup = json_map,
)]
#[bench::custom_borrowed()]
fn custom_borrowed_builtin(map: &'static HeaderMap) -> usize {
    let view = UserAgent::view(map).expect("valid user agent").expect("present user agent");
    expect_bytes(view.as_bytes(), USER_AGENT_VALUE)
}

#[metabench::benchmark(
    CUSTOM_BORROWED_CUSTOM,
    "custom",
    "custom_borrowed_custom",
    gungraun_setup = json_map,
)]
#[bench::custom_borrowed()]
fn custom_borrowed_custom(map: &'static HeaderMap) -> usize {
    let view = DownstreamAgent::view(map).expect("valid user agent").expect("present user agent");
    expect_bytes(view.as_bytes(), USER_AGENT_VALUE)
}

#[metabench::benchmark(
    CUSTOM_OWNED_BUILTIN,
    "custom",
    "custom_owned_builtin",
    gungraun_setup = json_map,
)]
#[bench::custom_owned()]
fn custom_owned_builtin(map: &'static HeaderMap) -> usize {
    let agent = UserAgent::owned(map).expect("valid user agent").expect("present user agent");
    let length = expect_bytes(agent.as_bytes(), USER_AGENT_VALUE);
    consume(agent);
    length
}

#[metabench::benchmark(
    CUSTOM_OWNED_CUSTOM,
    "custom",
    "custom_owned_custom",
    gungraun_setup = json_map,
)]
#[bench::custom_owned()]
fn custom_owned_custom(map: &'static HeaderMap) -> usize {
    let agent = DownstreamAgent::owned(map).expect("valid user agent").expect("present user agent");
    let length = expect_bytes(agent.0.as_bytes(), USER_AGENT_VALUE);
    consume(agent);
    length
}

#[metabench::benchmark(
    CUSTOM_OWNED_RAW_SOURCE,
    "custom",
    "custom_owned_raw_source",
    gungraun_setup = raw_user_agent_source,
)]
#[bench::custom_owned_raw_source()]
fn custom_owned_raw_source(source: RawUserAgentSource) -> usize {
    let agent = UserAgent::owned(&source).expect("valid user agent").expect("present user agent");
    let length = expect_bytes(agent.as_bytes(), USER_AGENT_VALUE);
    consume(agent);
    length
}

#[metabench::benchmark(
    CUSTOM_INSERT_BUILTIN,
    "custom",
    "custom_insert_builtin",
    gungraun_setup = our_user_agent,
    gungraun_teardown = drop_it,
)]
#[bench::custom_insert()]
fn custom_insert_builtin(state: (HeaderMap, UserAgentOwned)) -> (usize, HeaderMap) {
    let (mut map, agent) = state;
    expect_inserted(UserAgent::insert(&mut map, agent));
    let stored = map.get(&USER_AGENT).expect("inserted user agent");
    let length = expect_bytes(stored.as_bytes(), USER_AGENT_VALUE);
    (length, map)
}

#[metabench::benchmark(
    CUSTOM_INSERT_CUSTOM,
    "custom",
    "custom_insert_custom",
    gungraun_setup = downstream_agent,
    gungraun_teardown = drop_it,
)]
#[bench::custom_insert()]
fn custom_insert_custom(state: (HeaderMap, DownstreamAgent)) -> (usize, HeaderMap) {
    let (mut map, agent) = state;
    expect_inserted(DownstreamAgent::insert(&mut map, agent));
    let stored = map.get(&USER_AGENT).expect("inserted user agent");
    let length = expect_bytes(stored.as_bytes(), USER_AGENT_VALUE);
    (length, map)
}

const WARM_UP: Duration = Duration::from_secs(1);
const MEASUREMENT: Duration = Duration::from_secs(3);
const SAMPLES: usize = 60;

fn tuned_criterion() -> Criterion {
    Criterion::default()
        .warm_up_time(WARM_UP)
        .measurement_time(MEASUREMENT)
        .sample_size(SAMPLES)
}

macro_rules! register_case {
    ($group:ident, $identity:ident, $case:literal, $setup:path, $benchmark:path) => {
        $group.bench_with_input(BenchmarkId::new($identity.benchmark_name(), $case), &(), |bencher, &()| {
            bencher.iter_batched($setup, $benchmark, BatchSize::SmallInput);
        });
    };
}

macro_rules! register_group {
    (
        $criterion:ident,
        $name:literal,
        [$(($identity:ident, $case:literal, $setup:path, $benchmark:path)),+ $(,)?]
    ) => {
        let mut group = $criterion.benchmark_group(concat!("http_headers_micro/", $name));
        register_cases!(
            group,
            [$(($identity, $case, $setup, $benchmark)),+]
        );
        group.finish();
    };
}

macro_rules! register_cases {
    (
        $group:ident,
        [$(($identity:ident, $case:literal, $setup:path, $benchmark:path)),+ $(,)?]
    ) => {
        $(register_case!($group, $identity, $case, $setup, $benchmark);)+
    };
}

fn criterion_lookup(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "lookup",
        [
            (
                USER_AGENT_BORROWED_HTTP_HEADERS,
                "user_agent_borrowed",
                json_map,
                user_agent_borrowed_http_headers
            ),
            (
                USER_AGENT_BORROWED_HEADERS,
                "user_agent_borrowed",
                json_map,
                user_agent_borrowed_headers
            ),
            (
                USER_AGENT_OWNED_HTTP_HEADERS,
                "user_agent_owned",
                json_map,
                user_agent_owned_http_headers
            ),
            (USER_AGENT_OWNED_HEADERS, "user_agent_owned", json_map, user_agent_owned_headers),
            (
                USER_AGENT_ABSENT_HTTP_HEADERS,
                "user_agent_absent",
                cookies_4,
                user_agent_absent_http_headers
            ),
            (USER_AGENT_ABSENT_HEADERS, "user_agent_absent", cookies_4, user_agent_absent_headers),
        ]
    );
}

fn criterion_repeated(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "repeated",
        [
            (
                SET_COOKIE_BORROWED_1_HTTP_HEADERS,
                "set_cookie_borrowed_1",
                cookies_1,
                set_cookie_borrowed_1_http_headers
            ),
            (
                SET_COOKIE_BORROWED_1_HEADERS,
                "set_cookie_borrowed_1",
                cookies_1,
                set_cookie_borrowed_1_headers
            ),
            (
                SET_COOKIE_BORROWED_4_HTTP_HEADERS,
                "set_cookie_borrowed_4",
                cookies_4,
                set_cookie_borrowed_4_http_headers
            ),
            (
                SET_COOKIE_BORROWED_4_HEADERS,
                "set_cookie_borrowed_4",
                cookies_4,
                set_cookie_borrowed_4_headers
            ),
            (
                SET_COOKIE_BORROWED_12_HTTP_HEADERS,
                "set_cookie_borrowed_12",
                cookies_12,
                set_cookie_borrowed_12_http_headers
            ),
            (
                SET_COOKIE_BORROWED_12_HEADERS,
                "set_cookie_borrowed_12",
                cookies_12,
                set_cookie_borrowed_12_headers
            ),
            (
                SET_COOKIE_OWNED_4_HTTP_HEADERS,
                "set_cookie_owned_4",
                cookies_4,
                set_cookie_owned_4_http_headers
            ),
            (
                SET_COOKIE_OWNED_4_HEADERS,
                "set_cookie_owned_4",
                cookies_4,
                set_cookie_owned_4_headers
            ),
        ]
    );
}

fn criterion_list(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "list",
        [
            (
                CACHE_CONTROL_SINGLE_LINE_HTTP_HEADERS,
                "cache_control_single_line",
                json_map,
                cache_control_single_line_http_headers
            ),
            (
                CACHE_CONTROL_SINGLE_LINE_HEADERS,
                "cache_control_single_line",
                json_map,
                cache_control_single_line_headers
            ),
            (
                CACHE_CONTROL_MULTI_LINE_HTTP_HEADERS,
                "cache_control_multi_line",
                cache_multi_map,
                cache_control_multi_line_http_headers
            ),
            (
                CACHE_CONTROL_MULTI_LINE_HEADERS,
                "cache_control_multi_line",
                cache_multi_map,
                cache_control_multi_line_headers
            ),
            (
                CACHE_CONTROL_OWNED_HTTP_HEADERS,
                "cache_control_owned",
                json_map,
                cache_control_owned_http_headers
            ),
            (
                CACHE_CONTROL_OWNED_HEADERS,
                "cache_control_owned",
                json_map,
                cache_control_owned_headers
            ),
            (
                CACHE_CONTROL_ADVERSARIAL_HTTP_HEADERS,
                "cache_control_adversarial",
                adversarial_map,
                cache_control_adversarial_http_headers
            ),
            (
                CACHE_CONTROL_ADVERSARIAL_HEADERS,
                "cache_control_adversarial",
                adversarial_map,
                cache_control_adversarial_headers
            ),
            (
                CACHE_CONTROL_MAX_AGE_ONLY_HTTP_HEADERS,
                "cache_control_max_age_only",
                json_map,
                cache_control_max_age_only_http_headers
            ),
            (
                CACHE_CONTROL_MAX_AGE_ONLY_HEADERS,
                "cache_control_max_age_only",
                json_map,
                cache_control_max_age_only_headers
            ),
            (
                CACHE_CONTROL_ONE_DIRECTIVE_HTTP_HEADERS,
                "cache_control_one_directive",
                cache_one_map,
                cache_control_one_directive_http_headers
            ),
            (
                CACHE_CONTROL_ONE_DIRECTIVE_HEADERS,
                "cache_control_one_directive",
                cache_one_map,
                cache_control_one_directive_headers
            ),
        ]
    );
}

fn criterion_directives(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "directives",
        [
            (DIRECTIVE_VALUE_BYTES, "directive_value", json_map, directive_value_bytes),
            (DIRECTIVE_VALUE_STR, "directive_value", json_map, directive_value_str),
            (
                DIRECTIVE_VALUE_MANY_BYTES,
                "directive_value_many",
                adversarial_map,
                directive_value_many_bytes
            ),
            (
                DIRECTIVE_VALUE_MANY_STR,
                "directive_value_many",
                adversarial_map,
                directive_value_many_str
            ),
        ]
    );
}

fn criterion_structured(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "structured",
        [
            (
                CONTENT_TYPE_INSPECT_HTTP_HEADERS,
                "content_type_inspect",
                json_map,
                content_type_inspect_http_headers
            ),
            (
                CONTENT_TYPE_INSPECT_HEADERS,
                "content_type_inspect",
                json_map,
                content_type_inspect_headers
            ),
            (
                CONTENT_TYPE_PARAMETERS_HTTP_HEADERS,
                "content_type_parameters",
                content_type_many_map,
                content_type_parameters_http_headers
            ),
            (
                CONTENT_TYPE_PARAMETERS_HEADERS,
                "content_type_parameters",
                content_type_many_map,
                content_type_parameters_headers
            ),
            (
                CONTENT_TYPE_OWNED_HTTP_HEADERS,
                "content_type_owned",
                json_map,
                content_type_owned_http_headers
            ),
            (
                CONTENT_TYPE_OWNED_HEADERS,
                "content_type_owned",
                json_map,
                content_type_owned_headers
            ),
            (
                CONTENT_TYPE_MALFORMED_HTTP_HEADERS,
                "content_type_malformed",
                degraded_map,
                content_type_malformed_http_headers
            ),
            (
                CONTENT_TYPE_MALFORMED_HEADERS,
                "content_type_malformed",
                degraded_map,
                content_type_malformed_headers
            ),
        ]
    );
}

fn criterion_authorization(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "authorization",
        [
            (
                BASIC_DECODE_HTTP_HEADERS,
                "basic_decode",
                basic_credentials,
                basic_decode_http_headers
            ),
            (BASIC_DECODE_HEADERS, "basic_decode", basic_credentials, basic_decode_headers),
            (
                BASIC_VALIDATE_HTTP_HEADERS,
                "basic_validate",
                basic_map,
                basic_validate_http_headers
            ),
            (BASIC_VALIDATE_HEADERS, "basic_validate", basic_map, basic_validate_headers),
            (
                BEARER_BORROWED_HTTP_HEADERS,
                "bearer_borrowed",
                bearer_map,
                bearer_borrowed_http_headers
            ),
            (BEARER_BORROWED_HEADERS, "bearer_borrowed", bearer_map, bearer_borrowed_headers),
            (BASIC_ENCODE_HTTP_HEADERS, "basic_encode", empty_map, basic_encode_http_headers),
            (BASIC_ENCODE_HEADERS, "basic_encode", empty_map, basic_encode_headers),
        ]
    );
}

fn criterion_insertion(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "insertion",
        [
            (
                INSERT_USER_AGENT_HTTP_HEADERS,
                "insert_user_agent",
                our_user_agent,
                insert_user_agent_http_headers
            ),
            (
                INSERT_USER_AGENT_HEADERS,
                "insert_user_agent",
                their_user_agent,
                insert_user_agent_headers
            ),
            (
                INSERT_CONTENT_TYPE_HTTP_HEADERS,
                "insert_content_type",
                our_content_type,
                insert_content_type_http_headers
            ),
            (
                INSERT_CONTENT_TYPE_HEADERS,
                "insert_content_type",
                their_content_type,
                insert_content_type_headers
            ),
            (
                INSERT_CACHE_CONTROL_HTTP_HEADERS,
                "insert_cache_control",
                our_cache_control,
                insert_cache_control_http_headers
            ),
            (
                INSERT_CACHE_CONTROL_HEADERS,
                "insert_cache_control",
                their_cache_control,
                insert_cache_control_headers
            ),
        ]
    );
}

fn criterion_ownership(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "ownership",
        [
            (
                OWNERSHIP_USER_AGENT_MOVED,
                "ownership_user_agent",
                our_user_agent,
                ownership_user_agent_moved
            ),
            (
                OWNERSHIP_USER_AGENT_CLONED,
                "ownership_user_agent",
                our_user_agent,
                ownership_user_agent_cloned
            ),
            (
                OWNERSHIP_SET_COOKIE_MOVED,
                "ownership_set_cookie",
                our_cookies,
                ownership_set_cookie_moved
            ),
            (
                OWNERSHIP_SET_COOKIE_CLONED,
                "ownership_set_cookie",
                our_cookies,
                ownership_set_cookie_cloned
            ),
        ]
    );
}

fn criterion_credentials(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "credentials",
        [
            (
                CREDENTIALS_BASIC_REUSED,
                "credentials_basic",
                warm_credentials,
                credentials_basic_reused
            ),
            (
                CREDENTIALS_BASIC_HIGH_WATER_REUSED,
                "credentials_basic_high_water",
                high_water_warm_credentials,
                credentials_basic_high_water_reused
            ),
            (CREDENTIALS_BASIC_FRESH, "credentials_basic", basic_map, credentials_basic_fresh),
            (
                CREDENTIALS_LARGE_REUSED,
                "credentials_large",
                large_warm_credentials,
                credentials_large_reused
            ),
            (CREDENTIALS_LARGE_FRESH, "credentials_large", large_map, credentials_large_fresh),
        ]
    );
}

fn criterion_scanning_tokens(group: &mut BenchmarkGroup<'_, WallTime>) {
    register_cases!(
        group,
        [
            (TOKEN_15_DISPATCHED, "token_15", token_15, token_15_dispatched),
            (TOKEN_15_REFERENCE, "token_15", token_15, token_15_reference),
            (TOKEN_16_DISPATCHED, "token_16", token_16, token_16_dispatched),
            (TOKEN_16_REFERENCE, "token_16", token_16, token_16_reference),
            (TOKEN_31_DISPATCHED, "token_31", token_31, token_31_dispatched),
            (TOKEN_31_REFERENCE, "token_31", token_31, token_31_reference),
            (TOKEN_32_DISPATCHED, "token_32", token_32, token_32_dispatched),
            (TOKEN_32_REFERENCE, "token_32", token_32, token_32_reference),
            (TOKEN_33_DISPATCHED, "token_33", token_33, token_33_dispatched),
            (TOKEN_33_REFERENCE, "token_33", token_33, token_33_reference),
            (TOKEN_512_DISPATCHED, "token_512", token_512, token_512_dispatched),
            (TOKEN_512_REFERENCE, "token_512", token_512, token_512_reference),
        ]
    );
}

fn criterion_scanning_values(group: &mut BenchmarkGroup<'_, WallTime>) {
    register_cases!(
        group,
        [
            (TOKEN68_150_DISPATCHED, "token68_150", bearer_token_bytes, token68_150_dispatched),
            (TOKEN68_150_REFERENCE, "token68_150", bearer_token_bytes, token68_150_reference),
            (TOKEN68_512_DISPATCHED, "token68_512", token68_512_bytes, token68_512_dispatched),
            (TOKEN68_512_REFERENCE, "token68_512", token68_512_bytes, token68_512_reference),
            (
                FIELD_VALUE_150_DISPATCHED,
                "field_value_150",
                bearer_token_bytes,
                field_value_150_dispatched
            ),
            (
                FIELD_VALUE_150_REFERENCE,
                "field_value_150",
                bearer_token_bytes,
                field_value_150_reference
            ),
            (
                FIELD_VALUE_512_DISPATCHED,
                "field_value_512",
                field_value_bytes,
                field_value_512_dispatched
            ),
            (
                FIELD_VALUE_512_REFERENCE,
                "field_value_512",
                field_value_bytes,
                field_value_512_reference
            ),
            (
                DELIMITERS_SINGLE_LINE_DISPATCHED,
                "delimiters_single_line",
                single_line_list,
                delimiters_single_line_dispatched
            ),
            (
                DELIMITERS_SINGLE_LINE_REFERENCE,
                "delimiters_single_line",
                single_line_list,
                delimiters_single_line_reference
            ),
            (
                DELIMITERS_MULTI_LINE_DISPATCHED,
                "delimiters_multi_line",
                multi_line_list,
                delimiters_multi_line_dispatched
            ),
            (
                DELIMITERS_MULTI_LINE_REFERENCE,
                "delimiters_multi_line",
                multi_line_list,
                delimiters_multi_line_reference
            ),
        ]
    );
}

fn criterion_scanning(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("http_headers_micro/scanning");
    criterion_scanning_tokens(&mut group);
    criterion_scanning_values(&mut group);
    group.finish();
}

fn criterion_list_scanning(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "list_scanning",
        [
            (LIST_SCAN_15_DISPATCHED, "list_scan_15", list_scan_15, list_scan_15_dispatched),
            (LIST_SCAN_15_REFERENCE, "list_scan_15", list_scan_15, list_scan_15_reference),
            (LIST_SCAN_16_DISPATCHED, "list_scan_16", list_scan_16, list_scan_16_dispatched),
            (LIST_SCAN_16_REFERENCE, "list_scan_16", list_scan_16, list_scan_16_reference),
            (LIST_SCAN_17_DISPATCHED, "list_scan_17", list_scan_17, list_scan_17_dispatched),
            (LIST_SCAN_17_REFERENCE, "list_scan_17", list_scan_17, list_scan_17_reference),
            (LIST_SCAN_31_DISPATCHED, "list_scan_31", list_scan_31, list_scan_31_dispatched),
            (LIST_SCAN_31_REFERENCE, "list_scan_31", list_scan_31, list_scan_31_reference),
            (LIST_SCAN_32_DISPATCHED, "list_scan_32", list_scan_32, list_scan_32_dispatched),
            (LIST_SCAN_32_REFERENCE, "list_scan_32", list_scan_32, list_scan_32_reference),
            (LIST_SCAN_33_DISPATCHED, "list_scan_33", list_scan_33, list_scan_33_dispatched),
            (LIST_SCAN_33_REFERENCE, "list_scan_33", list_scan_33, list_scan_33_reference),
        ]
    );
}

fn criterion_uri_scanning(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "uri_scanning",
        [
            (URI_PATH_15_DISPATCHED, "uri_path_15", uri_path_15, uri_path_15_dispatched),
            (URI_PATH_15_REFERENCE, "uri_path_15", uri_path_15, uri_path_15_reference),
            (URI_PATH_16_DISPATCHED, "uri_path_16", uri_path_16, uri_path_16_dispatched),
            (URI_PATH_16_REFERENCE, "uri_path_16", uri_path_16, uri_path_16_reference),
            (URI_PATH_17_DISPATCHED, "uri_path_17", uri_path_17, uri_path_17_dispatched),
            (URI_PATH_17_REFERENCE, "uri_path_17", uri_path_17, uri_path_17_reference),
        ]
    );
}

fn criterion_forced_uri_backends(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "forced_uri_backends",
        [
            (URI_PATH_64_SSE2, "uri_path_64", uri_path_64, uri_path_64_sse2),
            (URI_PATH_64_SSSE3, "uri_path_64", uri_path_64, uri_path_64_ssse3),
            (URI_PATH_64_SSE42, "uri_path_64", uri_path_64, uri_path_64_sse42),
            (URI_PATH_64_NEON, "uri_path_64", uri_path_64, uri_path_64_neon),
        ]
    );
}

fn criterion_credential_scan(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "credential_scan",
        [
            (
                TOKEN68_CREDENTIAL_SHARED_SCANNER,
                "token68_credential",
                bearer_token_bytes,
                token68_credential_shared_scanner
            ),
            (
                TOKEN68_CREDENTIAL_MATCH_CHAIN,
                "token68_credential",
                bearer_token_bytes,
                token68_credential_match_chain
            ),
        ]
    );
}

fn criterion_custom(criterion: &mut Criterion) {
    register_group!(
        criterion,
        "custom",
        [
            (CUSTOM_BORROWED_BUILTIN, "custom_borrowed", json_map, custom_borrowed_builtin),
            (CUSTOM_BORROWED_CUSTOM, "custom_borrowed", json_map, custom_borrowed_custom),
            (CUSTOM_OWNED_BUILTIN, "custom_owned", json_map, custom_owned_builtin),
            (CUSTOM_OWNED_CUSTOM, "custom_owned", json_map, custom_owned_custom),
            (CUSTOM_INSERT_BUILTIN, "custom_insert", our_user_agent, custom_insert_builtin),
            (CUSTOM_INSERT_CUSTOM, "custom_insert", downstream_agent, custom_insert_custom),
        ]
    );
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    criterion_lookup(criterion);
    criterion_repeated(criterion);
    criterion_list(criterion);
    criterion_directives(criterion);
    criterion_structured(criterion);
    criterion_authorization(criterion);
    criterion_insertion(criterion);
    criterion_ownership(criterion);
    criterion_credentials(criterion);
    criterion_scanning(criterion);
    criterion_list_scanning(criterion);
    criterion_uri_scanning(criterion);
    criterion_forced_uri_backends(criterion);
    criterion_credential_scan(criterion);
    criterion_custom(criterion);
}

metabench::main!(
    criterion = {
        factory = tuned_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    groups = {
        LOOKUP {
            benchmarks = [
                USER_AGENT_BORROWED_HTTP_HEADERS,
                USER_AGENT_BORROWED_HEADERS,
                USER_AGENT_OWNED_HTTP_HEADERS,
                USER_AGENT_OWNED_HEADERS,
                USER_AGENT_ABSENT_HTTP_HEADERS,
                USER_AGENT_ABSENT_HEADERS,
            ],
            gungraun_compare_by_id = true,
        },
        REPEATED {
            benchmarks = [
                SET_COOKIE_BORROWED_1_HTTP_HEADERS,
                SET_COOKIE_BORROWED_1_HEADERS,
                SET_COOKIE_BORROWED_4_HTTP_HEADERS,
                SET_COOKIE_BORROWED_4_HEADERS,
                SET_COOKIE_BORROWED_12_HTTP_HEADERS,
                SET_COOKIE_BORROWED_12_HEADERS,
                SET_COOKIE_OWNED_4_HTTP_HEADERS,
                SET_COOKIE_OWNED_4_HEADERS,
            ],
            gungraun_compare_by_id = true,
        },
        LIST {
            benchmarks = [
                CACHE_CONTROL_SINGLE_LINE_HTTP_HEADERS,
                CACHE_CONTROL_SINGLE_LINE_HEADERS,
                CACHE_CONTROL_MULTI_LINE_HTTP_HEADERS,
                CACHE_CONTROL_MULTI_LINE_HEADERS,
                CACHE_CONTROL_OWNED_HTTP_HEADERS,
                CACHE_CONTROL_OWNED_HEADERS,
                CACHE_CONTROL_ADVERSARIAL_HTTP_HEADERS,
                CACHE_CONTROL_ADVERSARIAL_HEADERS,
                CACHE_CONTROL_MAX_AGE_ONLY_HTTP_HEADERS,
                CACHE_CONTROL_MAX_AGE_ONLY_HEADERS,
                CACHE_CONTROL_ONE_DIRECTIVE_HTTP_HEADERS,
                CACHE_CONTROL_ONE_DIRECTIVE_HEADERS,
            ],
            gungraun_compare_by_id = true,
        },
        DIRECTIVES {
            benchmarks = [
                DIRECTIVE_VALUE_BYTES,
                DIRECTIVE_VALUE_STR,
                DIRECTIVE_VALUE_MANY_BYTES,
                DIRECTIVE_VALUE_MANY_STR,
            ],
            gungraun_compare_by_id = true,
        },
        STRUCTURED {
            benchmarks = [
                CONTENT_TYPE_INSPECT_HTTP_HEADERS,
                CONTENT_TYPE_INSPECT_HEADERS,
                CONTENT_TYPE_PARAMETERS_HTTP_HEADERS,
                CONTENT_TYPE_PARAMETERS_HEADERS,
                CONTENT_TYPE_OWNED_HTTP_HEADERS,
                CONTENT_TYPE_OWNED_HEADERS,
                CONTENT_TYPE_MALFORMED_HTTP_HEADERS,
                CONTENT_TYPE_MALFORMED_HEADERS,
            ],
            gungraun_compare_by_id = true,
        },
        AUTHORIZATION {
            benchmarks = [
                BASIC_DECODE_HTTP_HEADERS,
                BASIC_DECODE_HEADERS,
                BASIC_VALIDATE_HTTP_HEADERS,
                BASIC_VALIDATE_HEADERS,
                BEARER_BORROWED_HTTP_HEADERS,
                BEARER_BORROWED_HEADERS,
                BASIC_ENCODE_HTTP_HEADERS,
                BASIC_ENCODE_HEADERS,
            ],
            gungraun_compare_by_id = true,
        },
        INSERTION {
            benchmarks = [
                INSERT_USER_AGENT_HTTP_HEADERS,
                INSERT_USER_AGENT_HEADERS,
                INSERT_CONTENT_TYPE_HTTP_HEADERS,
                INSERT_CONTENT_TYPE_HEADERS,
                INSERT_CACHE_CONTROL_HTTP_HEADERS,
                INSERT_CACHE_CONTROL_HEADERS,
            ],
            gungraun_compare_by_id = true,
        },
        OWNERSHIP {
            benchmarks = [
                OWNERSHIP_USER_AGENT_MOVED,
                OWNERSHIP_USER_AGENT_CLONED,
                OWNERSHIP_SET_COOKIE_MOVED,
                OWNERSHIP_SET_COOKIE_CLONED,
            ],
            gungraun_compare_by_id = true,
        },
        CREDENTIALS {
            benchmarks = [
                CREDENTIALS_BASIC_REUSED,
                CREDENTIALS_BASIC_HIGH_WATER_REUSED,
                CREDENTIALS_BASIC_FRESH,
                CREDENTIALS_LARGE_REUSED,
                CREDENTIALS_LARGE_FRESH,
            ],
            gungraun_compare_by_id = true,
        },
        SCANNING {
            benchmarks = [
                TOKEN_15_DISPATCHED,
                TOKEN_15_REFERENCE,
                TOKEN_16_DISPATCHED,
                TOKEN_16_REFERENCE,
                TOKEN_31_DISPATCHED,
                TOKEN_31_REFERENCE,
                TOKEN_32_DISPATCHED,
                TOKEN_32_REFERENCE,
                TOKEN_33_DISPATCHED,
                TOKEN_33_REFERENCE,
                TOKEN_512_DISPATCHED,
                TOKEN_512_REFERENCE,
                TOKEN68_150_DISPATCHED,
                TOKEN68_150_REFERENCE,
                TOKEN68_512_DISPATCHED,
                TOKEN68_512_REFERENCE,
                FIELD_VALUE_150_DISPATCHED,
                FIELD_VALUE_150_REFERENCE,
                FIELD_VALUE_512_DISPATCHED,
                FIELD_VALUE_512_REFERENCE,
                DELIMITERS_SINGLE_LINE_DISPATCHED,
                DELIMITERS_SINGLE_LINE_REFERENCE,
                DELIMITERS_MULTI_LINE_DISPATCHED,
                DELIMITERS_MULTI_LINE_REFERENCE,
                COMMA_ITEMS_IRRELEVANT_BYTES,
            ],
            gungraun_compare_by_id = true,
        },
        LIST_SCANNING {
            benchmarks = [
                LIST_SCAN_15_DISPATCHED,
                LIST_SCAN_15_REFERENCE,
                LIST_SCAN_16_DISPATCHED,
                LIST_SCAN_16_REFERENCE,
                LIST_SCAN_17_DISPATCHED,
                LIST_SCAN_17_REFERENCE,
                LIST_SCAN_31_DISPATCHED,
                LIST_SCAN_31_REFERENCE,
                LIST_SCAN_32_DISPATCHED,
                LIST_SCAN_32_REFERENCE,
                LIST_SCAN_33_DISPATCHED,
                LIST_SCAN_33_REFERENCE,
            ],
            gungraun_compare_by_id = true,
        },
        URI_SCANNING {
            benchmarks = [
                URI_PATH_15_DISPATCHED,
                URI_PATH_15_REFERENCE,
                URI_PATH_16_DISPATCHED,
                URI_PATH_16_REFERENCE,
                URI_PATH_17_DISPATCHED,
                URI_PATH_17_REFERENCE,
            ],
            gungraun_compare_by_id = true,
        },
        FORCED_URI_BACKENDS {
            benchmarks = [
                URI_PATH_64_SSE2,
                URI_PATH_64_SSSE3,
                URI_PATH_64_SSE42,
                URI_PATH_64_NEON,
            ],
        },
        CREDENTIAL_SCAN {
            benchmarks = [
                TOKEN68_CREDENTIAL_SHARED_SCANNER,
                TOKEN68_CREDENTIAL_MATCH_CHAIN,
            ],
            gungraun_compare_by_id = true,
        },
        CUSTOM {
            benchmarks = [
                CUSTOM_BORROWED_BUILTIN,
                CUSTOM_BORROWED_CUSTOM,
                CUSTOM_OWNED_BUILTIN,
                CUSTOM_OWNED_CUSTOM,
                CUSTOM_OWNED_RAW_SOURCE,
                CUSTOM_INSERT_BUILTIN,
                CUSTOM_INSERT_CUSTOM,
            ],
            gungraun_compare_by_id = true,
        },
    },
);
