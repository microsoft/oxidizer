// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ConfigurationError`: every dynamic-route registration failure a generated builder
//! reports, and the aggregated `Display` that surfaces all of them at once.

#![allow(
    clippy::literal_string_with_formatting_args,
    reason = "route path templates use `{var}` capture syntax, not string formatting"
)]

use std::error::Error as _;

use routerama::{HttpMethod, resolver};

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum Api {
    Home,
    Book { book: String },
}

#[test]
fn a_missing_add_call_is_reported() {
    let error = Api::builder()
        .add_home(HttpMethod::GET, "/")
        .build()
        .expect_err("add_book was never called");
    let text = error.to_string();
    assert!(text.contains("add_book"), "{text}");
    assert!(text.contains("was never called"), "{text}");
    assert!(text.contains("Book"), "{text}");
    assert_eq!(error.invalid_http_method_value(), None);
    assert!(error.source().is_none());
}

#[test]
fn an_invalid_path_is_reported() {
    let error = Api::builder()
        .add_home(HttpMethod::GET, "/")
        .add_book(HttpMethod::GET, "/books/{")
        .build()
        .expect_err("the path template does not parse");
    let text = error.to_string();
    assert!(text.contains("invalid path"), "{text}");
    assert!(text.contains("/books/{"), "{text}");

    let source = error
        .source()
        .and_then(|source| source.downcast_ref::<http_path_template::ParseError>())
        .expect("the path-template parse error is retained");
    assert!(source.is_unbalanced_braces());
}

#[test]
fn every_invalid_template_cause_is_retained() {
    let error = Api::builder()
        .add_home(HttpMethod::GET, "/")
        .add_book(HttpMethod::GET, "/books/{")
        .add_book(HttpMethod::GET, "books/{book}")
        .build()
        .expect_err("both path templates are invalid");

    let causes: Vec<&http_path_template::ParseError> = error
        .causes()
        .map(|cause| {
            cause
                .downcast_ref::<http_path_template::ParseError>()
                .expect("invalid-template causes retain their concrete type")
        })
        .collect();
    assert_eq!(causes.len(), 2);
    assert!(causes[0].is_unbalanced_braces());
    assert!(causes[1].is_missing_leading_slash());
}

#[test]
fn mismatched_captures_are_reported() {
    let error = Api::builder()
        .add_home(HttpMethod::GET, "/")
        .add_book(HttpMethod::GET, "/books/{wrong}")
        .build()
        .expect_err("the path captures do not match the variant fields");
    let text = error.to_string();
    assert!(text.contains("do not match"), "{text}");
    assert!(text.contains("wrong"), "{text}");
    assert!(text.contains("book"), "{text}");
}

#[test]
fn extra_captures_are_reported() {
    let error = Api::builder()
        .add_home(HttpMethod::GET, "/")
        .add_book(HttpMethod::GET, "/books/{book}/{extra}")
        .build()
        .expect_err("the path captures more fields than the variant");
    let text = error.to_string();
    assert!(text.contains("do not match"), "{text}");
    assert!(text.contains("extra"), "{text}");
    assert!(text.contains("book"), "{text}");
}

#[test]
fn every_failure_surfaces_at_once() {
    let error = Api::builder().build().expect_err("neither add_home nor add_book was called");
    let text = error.to_string();
    assert!(text.starts_with("failed to build resolver:"), "{text}");
    assert!(text.contains("add_home"), "{text}");
    assert!(text.contains("add_book"), "{text}");
    assert_eq!(text.matches("\n  - ").count(), 2, "{text}");
}

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum Pair {
    Two { left: String, right: String },
}

#[test]
fn a_mismatch_between_an_empty_and_a_multi_capture_set_is_reported() {
    let error = Pair::builder()
        .add_two(HttpMethod::GET, "/pair")
        .build()
        .expect_err("the path captures do not match the variant fields");
    let text = error.to_string();
    assert!(text.contains("{}"), "{text}");
    assert!(text.contains("{left, right}"), "{text}");
}

#[test]
fn conflicting_dynamic_routes_are_reported_instead_of_using_registration_order() {
    #[resolver]
    enum Conflicting {
        First,
        Second,
    }

    let error = Conflicting::builder()
        .add_first(HttpMethod::GET, "/same")
        .add_second(HttpMethod::GET, "/same")
        .build()
        .expect_err("both routes match the same requests");
    let text = error.to_string();
    assert!(text.contains("conflicting routes"), "{text}");
    assert!(text.contains("First, Second"), "{text}");
}
