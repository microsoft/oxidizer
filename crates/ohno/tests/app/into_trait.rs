// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `IntoAppError` trait implementations.

use ohno::{AppError, IntoAppError};
use ohno::assert_error_message;

#[test]
fn result_into_app_err() {
    fn parse_number(s: &str) -> Result<i32, AppError> {
        s.parse::<i32>().into_app_err("failed to parse number")
    }

    let err = parse_number("xyz").unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    assert!(msg.contains("failed to parse number"));
}

#[test]
fn result_into_app_err_with() {
    fn parse_with_context(s: &str) -> Result<i32, AppError> {
        s.parse::<i32>().into_app_err_with(|| format!("failed to parse: {s}"))
    }

    let err = parse_with_context("abc").unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    assert!(msg.contains("failed to parse: abc"));
}

#[test]
fn option_into_app_err() {
    fn make_error() -> Result<i32, AppError> {
        None.into_app_err("value not found")
    }

    let err = make_error().unwrap_err();
    assert_error_message!(err, "value not found");
    assert!(err.source().is_none());
}

#[test]
fn option_into_app_err_with() {
    fn with_context() -> Result<i32, AppError> {
        None.into_app_err_with(|| "nothing found")
    }

    let err = with_context().unwrap_err();
    assert_error_message!(err, "nothing found");
    assert!(err.source().is_none());
}

#[test]
fn ohno_on_into_app_err_error() {
    fn level1() -> Result<i32, AppError> {
        Err(AppError::new("root error"))
    }

    fn level2() -> Result<i32, AppError> {
        level1().into_app_err("context added")
    }

    fn level3() -> Result<i32, AppError> {
        level2().into_app_err_with(|| "more context added")
    }

    let err = level3().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("root error"), "{msg}");
    assert!(msg.contains("> context added"), "{msg}");
    assert!(msg.contains("> more context added"), "{msg}");
}

#[test]
fn string_ref() {
    fn fail() -> Result<i32, AppError> {
        let context = String::from("failed operation");
        None.into_app_err(&context)
    }

    let err = fail().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("failed operation"));
}
