// Copyright (c) Microsoft Corporation.

//! Tests for OhWell trait extension methods.

use ohno::app::{AppError, IntoAppError, Result};
use ohno::assert_error_message;

#[test]
fn result_ohno() {
    fn parse_number(s: &str) -> Result<i32> {
        s.parse::<i32>().ohno("failed to parse number")
    }

    let err = parse_number("xyz").unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    assert!(msg.contains("failed to parse number"));
}

#[test]
fn result_ohno_with() {
    fn parse_with_context(s: &str) -> Result<i32> {
        s.parse::<i32>().ohno_with(|| format!("failed to parse: {}", s))
    }

    let err = parse_with_context("abc").unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    assert!(msg.contains("failed to parse: abc"));
}

#[test]
fn option_ohno() {
    fn make_error() -> Result<i32> {
        None.ohno("value not found")
    }

    let err = make_error().unwrap_err();
    assert_error_message!(err, "value not found");
    assert!(err.source().is_none());
}

#[test]
fn option_ohno_with() {
    fn with_context() -> Result<i32> {
        None.ohno_with(|| "nothing found")
    }

    let err = with_context().unwrap_err();
    assert_error_message!(err, "nothing found");
    assert!(err.source().is_none());
}

#[test]
fn ohno_on_ohno_error() {
    fn level1() -> Result<i32> {
        Err(AppError::new("root error"))
    }

    fn level2() -> Result<i32> {
        level1().ohno("context added")
    }

    let err = level2().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("root error"));
    assert!(msg.contains("context added"));
}
