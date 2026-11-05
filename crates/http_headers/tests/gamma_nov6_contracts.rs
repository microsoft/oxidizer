// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exact wire and error contracts for header construction.

#![cfg(all(feature = "headers-range", feature = "headers-cache-control", feature = "headers-websocket"))]

use http_headers::headers::{AcceptRangesOwned, CacheControlOwned, SecWebSocketExtensionsOwned};
use http_headers::{DecodeErrorKind, FieldName};

#[test]
fn gamma_nov6_accept_ranges_does_not_add_a_leading_separator() {
    let value = AcceptRangesOwned::from_units(["items", "records"]).unwrap();
    assert_eq!(value.to_string(), "items, records");
    assert_eq!(value.units().collect::<Vec<_>>(), ["items", "records"]);
}

#[test]
fn gamma_nov6_empty_builders_preserve_the_exact_syntax_error() {
    let cache = CacheControlOwned::builder().build().unwrap_err();
    assert_eq!(cache.header(), &FieldName::CacheControl);
    assert_eq!(cache.kind(), DecodeErrorKind::InvalidSyntax);
    let extensions = SecWebSocketExtensionsOwned::builder().build().unwrap_err();
    assert_eq!(extensions.header(), &FieldName::SecWebSocketExtensions);
    assert_eq!(extensions.kind(), DecodeErrorKind::InvalidSyntax);
}
