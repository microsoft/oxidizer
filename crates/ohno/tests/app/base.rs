// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `AppError` methods.

use ohno::{EnrichableExt, AppError, assert_error_message};

#[test]
fn source_none() {
    let app_err = AppError::new("an error occurred");
    assert!(app_err.source().is_none(), "{app_err:?}");
}

#[test]
fn source_some() {
    let inner_err = std::io::Error::other("inner error");
    let app_err = AppError::new(inner_err);
    let source = app_err.source().expect("expected a source error");
    let io_err = source.downcast_ref::<std::io::Error>().expect("expected std::io::Error");
    assert_eq!(io_err.to_string(), "inner error");
}

#[test]
fn find_source() {
    #[ohno::error]
    struct ErrorA;

    #[ohno::error]
    struct ErrorB;

    #[ohno::error]
    struct ErrorC;

    let app_err = AppError::new(ErrorA::caused_by(ErrorB::caused_by("an error occurred")));

    let _ = app_err.find_source::<ErrorA>().expect("expected to find ErrorA");
    let _ = app_err.find_source::<ErrorB>().expect("expected to find ErrorB");

    let err_c = app_err.find_source::<ErrorC>();
    assert!(err_c.is_none(), "{err_c:?}");
}

#[test]
fn debug_equal_to_display() {
    let app_err = AppError::new("an error occurred");
    let debug_str = format!("{app_err:?}");
    let display_str = format!("{app_err}");
    assert_eq!(debug_str, display_str);
}

#[test]
fn as_std_error_ref() {
    let app_err = AppError::new("an error occurred");
    let std_err: &dyn std::error::Error = app_err.as_ref();
    assert_error_message!(std_err, "an error occurred");
}

#[test]
fn backtrace_method() {
    let app_err = AppError::new("an error occurred");
    let _backtrace = app_err.backtrace();
    // it depends on whether backtraces are enabled in the environment
}

#[test]
fn is_cloneable() {
    let app_err = AppError::new("an error occurred");
    let app_err_clone = app_err.clone();
    assert_eq!(app_err.to_string(), app_err_clone.to_string());

    let app_err = app_err.enrich("additional context").enrich("more context");
    let app_err_str = app_err.to_string();
    assert!(app_err_str.contains("> additional context"));
    assert!(app_err_str.contains("> more context"));
    assert_ne!(app_err_str, app_err_clone.to_string());

    let app_err_clone2 = app_err.clone();
    assert_eq!(app_err_str, app_err_clone2.to_string());
}
