// Copyright (c) Microsoft Corporation.

//! Tests for `AppError::new` construction.

use ohno::app::AppError;
use ohno::assert_error_message;

#[test]
fn error_new_with_string() {
    let err = AppError::new("connection failed");
    assert_error_message!(err, "connection failed");
}

#[test]
fn error_new_with_owned_string() {
    let err = AppError::new(String::from("owned message"));
    assert_error_message!(err, "owned message");
}

#[test]
fn wrap_io_error_with_new() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "config.toml");
    let wrapped = AppError::new(io_err);
    assert_error_message!(wrapped, "config.toml");
}

#[test]
fn wrap_parse_error() {
    let parse_err = "not_a_number".parse::<i32>().unwrap_err();
    let wrapped = AppError::new(parse_err);
    assert_error_message!(wrapped, "invalid digit found in string");
}
