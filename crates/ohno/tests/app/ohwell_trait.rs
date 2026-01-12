// Copyright (c) Microsoft Corporation.

//! Tests for OhWell trait extension methods.

use ohno::app::{AppError, OhWell, Result};
use ohno::{assert_error_message};

#[test]
fn result_ohwell() {
    fn parse_number(s: &str) -> Result<i32> {
        s.parse::<i32>().ohwell("failed to parse number")
    }

    let err = parse_number("xyz").unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    assert!(msg.contains("failed to parse number"));
}

#[test]
fn result_ohwell_with() {
    fn parse_with_context(s: &str) -> Result<i32> {
        s.parse::<i32>()
            .ohwell_with(|| format!("failed to parse: {}", s))
    }

    let err = parse_with_context("abc").unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    assert!(msg.contains("failed to parse: abc"));
}

#[test]
fn option_ohwell() {
    fn make_error() -> Result<i32> {
        None.ohwell("value not found")
    }

    let err = make_error().unwrap_err();
    assert_error_message!(err, "value not found");
    assert!(err.source().is_none());
}

#[test]
fn option_ohwell_with() {
    fn with_context() -> Result<i32> {
        None.ohwell_with(|| "nothing found")
    }

    let err = with_context().unwrap_err();
    assert_error_message!(err, "nothing found");
    assert!(err.source().is_none());
}

#[test]
fn ohwell_on_ohwell_error() {
    fn level1() -> Result<i32> {
        Err(AppError::new("root error"))
    }

    fn level2() -> Result<i32> {
        level1().ohwell("context added")
    }

    let err = level2().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("root error"));
    assert!(msg.contains("context added"));
}
