// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Immutable name corpora shared by recognition benchmarks and ownership tests.
//!
//! Each parsed corpus is initialized once, so repeated setup reuses its owned names.

use std::sync::LazyLock;

use http_headers::FieldName;

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

pub(super) fn known_names() -> &'static [&'static [u8]] {
    KNOWN_NAMES
}

pub(super) fn custom_names_lowercase() -> &'static [&'static [u8]] {
    assert_eq!(CUSTOM_NAMES_LOWERCASE.len(), CUSTOM_NAMES_MIXED_CASE.len());
    assert!(
        CUSTOM_NAMES_LOWERCASE
            .iter()
            .zip(CUSTOM_NAMES_MIXED_CASE)
            .all(|(lowercase, mixed_case)| lowercase.eq_ignore_ascii_case(mixed_case))
    );
    CUSTOM_NAMES_LOWERCASE
}

pub(super) fn custom_names_mixed_case() -> &'static [&'static [u8]] {
    CUSTOM_NAMES_MIXED_CASE
}

pub(super) fn custom_header_names() -> &'static [FieldName] {
    static NAMES: LazyLock<Vec<FieldName>> = LazyLock::new(|| {
        CUSTOM_NAMES_MIXED_CASE
            .iter()
            .map(|name| FieldName::try_from_bytes(name).expect("valid field name"))
            .collect()
    });
    NAMES.as_slice()
}

pub(super) fn http_names() -> &'static [http::HeaderName] {
    static NAMES: LazyLock<Vec<http::HeaderName>> = LazyLock::new(|| {
        KNOWN_NAMES
            .iter()
            .chain(CUSTOM_NAMES_MIXED_CASE)
            .map(|name| http::HeaderName::from_bytes(name).expect("valid field name"))
            .collect()
    });
    NAMES.as_slice()
}

pub(super) fn crate_names() -> &'static [FieldName] {
    static NAMES: LazyLock<Vec<FieldName>> = LazyLock::new(|| {
        KNOWN_NAMES
            .iter()
            .chain(CUSTOM_NAMES_MIXED_CASE)
            .map(|name| FieldName::try_from_bytes(name).expect("valid field name"))
            .collect()
    });
    NAMES.as_slice()
}
