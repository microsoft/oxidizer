// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `IntoAppError` trait implementations.

use ohno::assert_error_message;
use ohno::{AppError, IntoAppError};

#[test]
fn result_into_app_err() {
    fn parse_number(s: &str) -> (Result<i32, AppError>, u32) {
        (s.parse::<i32>().into_app_err("failed to parse number"), line!())
    }

    let (result, call_line) = parse_number("xyz");
    let err = result.unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    let expected_location = format!("{}:{}", file!(), call_line);
    let lines = msg.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "invalid digit found in string");
    assert!(lines[1].starts_with("> failed to parse number (at "), "{msg}");
    assert!(lines[1].contains(&expected_location), "{msg}");
}

#[test]
fn result_into_app_err_with() {
    fn parse_with_context(s: &str) -> (Result<i32, AppError>, u32) {
        (s.parse::<i32>().into_app_err_with(|| format!("failed to parse: {s}")), line!())
    }

    let (result, call_line) = parse_with_context("abc");
    let err = result.unwrap_err();
    assert_error_message!(err, "invalid digit found in string");
    let msg = err.to_string();
    let expected_location = format!("{}:{}", file!(), call_line);
    assert!(msg.contains("failed to parse: abc"), "{msg}");
    assert!(msg.contains(&expected_location), "{msg}");
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

    fn level2() -> (Result<i32, AppError>, u32) {
        (level1().into_app_err("context added"), line!())
    }

    fn level3(inner: Result<i32, AppError>) -> (Result<i32, AppError>, u32) {
        (inner.into_app_err_with(|| "more context added"), line!())
    }

    let (result2, line2) = level2();
    let (result3, line3) = level3(result2);
    let err = result3.unwrap_err();
    let msg = err.to_string();
    let expected_location2 = format!("{}:{}", file!(), line2);
    let expected_location3 = format!("{}:{}", file!(), line3);
    assert!(msg.contains("root error"), "{msg}");
    assert!(msg.contains("> context added"), "{msg}");
    assert!(msg.contains("> more context added"), "{msg}");
    assert!(msg.contains(&expected_location2), "{msg}");
    assert!(msg.contains(&expected_location3), "{msg}");
}

#[test]
fn string_ref() {
    fn fail() -> Result<i32, AppError> {
        let context = String::from("failed operation");
        None.into_app_err(&context)
    }

    let err = fail().unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("failed operation"), "{msg}");
}

#[test]
fn result_into_app_err_ok() {
    let result: Result<i32, std::num::ParseIntError> = "42".parse();
    let ok = result.into_app_err("should not appear").unwrap();
    assert_eq!(ok, 42);
}

#[test]
fn result_into_app_err_with_ok() {
    let result: Result<i32, std::num::ParseIntError> = "42".parse();
    let ok = result.into_app_err_with(|| "should not appear").unwrap();
    assert_eq!(ok, 42);
}

#[test]
fn option_into_app_err_ok() {
    let ok = Some(42).into_app_err("should not appear").unwrap();
    assert_eq!(ok, 42);
}

#[test]
fn option_into_app_err_with_ok() {
    let ok = Some(42).into_app_err_with(|| "should not appear").unwrap();
    assert_eq!(ok, 42);
}

#[test]
fn app_error_result_into_app_err_ok() {
    let result: Result<i32, AppError> = Ok(42);
    let ok = result.into_app_err("should not appear").unwrap();
    assert_eq!(ok, 42);
}

#[test]
fn app_error_result_into_app_err_with_ok() {
    let result: Result<i32, AppError> = Ok(42);
    let ok = result.into_app_err_with(|| "should not appear").unwrap();
    assert_eq!(ok, 42);
}
