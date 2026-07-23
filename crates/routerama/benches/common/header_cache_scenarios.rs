// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::hint::black_box;

use http::HeaderMap;
use http::header::{ACCEPT_ENCODING, DATE, HeaderValue};
use routerama::route::header::{AcceptEncoding, Date, HeaderCache, HeaderExt as _};

/// A realistic `Date` field: an IMF-fixdate that repeats verbatim across the
/// stream of requests a worker sees, which is the case the cache targets.
const DATE_VALUE: &str = "Sun, 06 Nov 1994 08:49:37 GMT";

/// A realistic weighted content-coding preference list.
const ACCEPT_ENCODING_VALUE: &str =
    "br, gzip;q=0.9, deflate;q=0.8, identity;q=0.5, *;q=0.1";

/// Builds a one-field `HeaderMap` carrying the shared `Date` value.
fn date_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(DATE, HeaderValue::from_static(DATE_VALUE));
    headers
}

/// Builds a one-field map carrying the shared `Accept-Encoding` value.
fn accept_encoding_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static(ACCEPT_ENCODING_VALUE));
    headers
}

/// Returns a `HeaderCache` already warmed with the shared `Date` value, so the
/// measured lookup exercises the steady-state cache-hit path.
fn warm_date_cache() -> (HeaderCache, HeaderMap) {
    let headers = date_headers();
    let mut cache = HeaderCache::new();
    let _ = cache.date(&headers);
    (cache, headers)
}

/// Returns a cache warmed with the shared `Accept-Encoding` value.
fn warm_accept_encoding_cache() -> (HeaderCache, HeaderMap) {
    let headers = accept_encoding_headers();
    let mut cache = HeaderCache::new();
    let _ = cache.accept_encoding(&headers);
    (cache, headers)
}

/// Parses `Date` from scratch on every call through the stateless accessor.
fn date_uncached(headers: &HeaderMap) -> Option<Date> {
    black_box(headers.date())
}

/// Resolves `Date` through a warm cache, hitting the memoized value.
fn date_cached(cache: &mut HeaderCache, headers: &HeaderMap) -> Option<Date> {
    black_box(cache.date(black_box(headers)))
}

/// Parses `Accept-Encoding` from scratch.
fn accept_encoding_uncached(headers: &HeaderMap) -> Option<AcceptEncoding> {
    black_box(headers.accept_encoding())
}

/// Resolves `Accept-Encoding` through a warm cache.
fn accept_encoding_cached(
    cache: &mut HeaderCache,
    headers: &HeaderMap,
) -> Option<AcceptEncoding> {
    black_box(cache.accept_encoding(black_box(headers)))
}
