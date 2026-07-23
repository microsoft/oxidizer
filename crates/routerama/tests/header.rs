// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed header accessors exercised through the public `route::header` API.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http::header::{ACCEPT_ENCODING, CACHE_CONTROL, DATE, EXPIRES, HeaderName, LAST_MODIFIED};
use http::{HeaderMap, HeaderValue};
use routerama::route::header::{Encoding, HeaderExt as _};

fn headers(name: HeaderName, value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(name, HeaderValue::from_str(value).expect("valid header value"));
    headers
}

#[test]
fn typed_accessors_parse_each_field() {
    let map = headers(LAST_MODIFIED, "Sun, 06 Nov 1994 08:49:37 GMT");
    let parsed: SystemTime = map.last_modified().expect("date").into();
    assert_eq!(
        parsed.duration_since(UNIX_EPOCH).expect("date is after the epoch"),
        Duration::from_secs(784_111_777)
    );

    let map = headers(CACHE_CONTROL, "no-store");
    assert!(map.cache_control().expect("cache-control").no_store());

    let map = headers(ACCEPT_ENCODING, "br");
    assert!(map.accept_encoding().expect("encoding").accepts(Encoding::Brotli));
}

#[test]
fn date_accessors_reject_absent_repeated_and_malformed_fields() {
    assert!(HeaderMap::new().date().is_none());
    assert!(headers(DATE, "not a date").date().is_none());
    assert!(headers(DATE, "Sun, 31 Feb 2024 08:49:37 GMT").date().is_none());

    let mut repeated = headers(EXPIRES, "Sun, 06 Nov 1994 08:49:37 GMT");
    repeated.append(EXPIRES, HeaderValue::from_static("Sun, 06 Nov 1994 08:49:38 GMT"));
    assert!(repeated.expires().is_none());
}

#[test]
fn cache_control_combines_repeated_lines() {
    let mut map = headers(CACHE_CONTROL, "max-age=60");
    map.append(CACHE_CONTROL, HeaderValue::from_static("public"));

    let control = map.cache_control().expect("valid combined field");
    assert_eq!(control.max_age(), Some(Duration::from_secs(60)));
    assert!(control.public());
}

#[test]
fn cache_control_directive_names_are_matched_case_insensitively() {
    let map = headers(CACHE_CONTROL, "No-Store, NO-CACHE, Max-Age=30");

    let control = map.cache_control().expect("valid field");
    assert!(control.no_store());
    assert!(control.no_cache());
    assert_eq!(control.max_age(), Some(Duration::from_secs(30)));
}

#[test]
fn cache_control_ignores_only_the_directives_it_cannot_parse() {
    let map = headers(CACHE_CONTROL, "no-store, max-age=abc, bogus directive, public");

    let control = map.cache_control().expect("the field survives unparsable directives");
    assert!(control.no_store());
    assert!(control.public());
    assert_eq!(control.max_age(), None);
}

#[test]
fn cache_control_reads_quoted_string_arguments() {
    let map = headers(CACHE_CONTROL, "private=\"set-cookie, authorization\", no-cache=\"set-cookie\"");

    let control = map.cache_control().expect("valid field");
    assert!(control.private());
    assert!(control.no_cache());
}

#[test]
fn cache_control_does_not_read_directives_inside_quoted_strings() {
    let map = headers(CACHE_CONTROL, "private=\"x, no-store\", max-age=5");

    let control = map.cache_control().expect("valid field");
    assert!(control.private());
    assert!(!control.no_store());
    assert_eq!(control.max_age(), Some(Duration::from_secs(5)));
}

#[test]
fn accept_encoding_combines_repeated_lines() {
    let mut map = headers(ACCEPT_ENCODING, "gzip;q=0.2");
    map.append(ACCEPT_ENCODING, HeaderValue::from_static("br"));

    let decision = map.accept_encoding().expect("valid combined field");
    assert_eq!(decision.quality(Encoding::Gzip), 200);
    assert!(decision.accepts(Encoding::Brotli));
}
