// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Capture-related `ResolveError` accessors and formatting, plus an encoding
//! error surfacing from a live resolve.

#![allow(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use routerama::{ResolveError, resolver};

#[test]
fn field_returns_the_offending_field_for_every_variant() {
    assert_eq!(ResolveError::MissingCapture("a").field(), Some("a"));
    assert_eq!(ResolveError::InvalidCapture("b").field(), Some("b"));
    assert_eq!(ResolveError::UndecodableCapture("c").field(), Some("c"));
}

#[test]
fn display_renders_each_variant() {
    assert_eq!(
        ResolveError::InvalidPath("/books?sort=title").to_string(),
        "expected a URI path without a query or fragment, got `/books?sort=title`"
    );
    assert_eq!(ResolveError::MissingCapture("a").to_string(), "missing capture for field `a`");
    assert_eq!(
        ResolveError::InvalidCapture("b").to_string(),
        "failed to parse capture for field `b`"
    );
    assert_eq!(
        ResolveError::UndecodableCapture("c").to_string(),
        "failed to percent-decode capture for field `c`"
    );
}

#[resolver]
#[derive(Debug)]
enum Route {
    #[route(GET, "/books/{book}")]
    GetBook { book: String },
}

#[test]
fn a_malformed_escape_resolves_to_a_decode_error() {
    let resolver = Route::resolver();
    assert!(matches!(
        resolver.resolve("GET", "/books/%2"),
        Err(ResolveError::UndecodableCapture("book"))
    ));
}

#[test]
fn a_well_formed_capture_decodes_successfully() {
    let resolver = Route::resolver();
    match resolver.resolve("GET", "/books/a%20b") {
        Ok(Route::GetBook { book }) => assert_eq!(book, "a b"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn resolve_error_accessors_display_and_source_are_consistent() {
    use std::error::Error as _;

    let not_found = ResolveError::NotFound("/missing");
    assert_eq!(not_found.path(), Some("/missing"));
    assert_eq!(not_found.field(), None);
    assert_eq!(not_found.to_string(), "no route matched path `/missing`");
    assert!(not_found.source().is_none());

    let invalid_path = ResolveError::InvalidPath("/books#reviews");
    assert_eq!(invalid_path.path(), Some("/books#reviews"));
    assert_eq!(invalid_path.field(), None);
    assert!(invalid_path.source().is_none());

    let capture = ResolveError::InvalidCapture("book");
    assert_eq!(capture.path(), None);
    assert_eq!(capture.field(), Some("book"));
    assert_eq!(capture.to_string(), "failed to parse capture for field `book`");
    assert!(capture.source().is_none());
}
