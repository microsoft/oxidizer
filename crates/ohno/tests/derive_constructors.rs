// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test for the constructor methods in derive(Error) macro

use std::io;

use ohno::{Error, OhnoCore, assert_error_message};

#[derive(Error)]
struct SimpleError {
    #[error]
    inner: OhnoCore,
}

#[derive(Error)]
struct ConfigError {
    path: String,
    #[error]
    inner: OhnoCore,
}

#[derive(Error)]
struct CustomError {
    message: String,
    #[error]
    inner: OhnoCore,
}

#[test]
fn test_simple_error_constructors() {
    // Test new method
    let error = SimpleError::new();
    assert_error_message!(error, "SimpleError");

    // Test caused_by method
    let error = SimpleError::caused_by("test error");
    assert!(format!("{error}").contains("test error"));
}

#[test]
fn test_config_error_constructors() {
    // Test new method
    let error = ConfigError::new("/etc/config");
    assert_eq!(error.path, "/etc/config");
    assert_error_message!(error, "ConfigError");
}

#[test]
fn test_custom_error_constructors() {
    // Test new method
    let error = CustomError::new("hello");
    assert_eq!(error.message, "hello");

    // Test caused_by method
    let io_error = io::Error::new(io::ErrorKind::NotFound, "file.txt");
    let error = CustomError::caused_by("hello", io_error);
    assert_eq!(error.message, "hello");
    assert!(format!("{error}").contains("file.txt"));
}

#[test]
fn test_caused_by_accepts_various_types() {
    // String slice
    let error = SimpleError::caused_by("string error");
    assert!(error.to_string().contains("string error"));

    // io::Error
    let io_error = io::Error::new(io::ErrorKind::NotFound, "file not found");
    let error = SimpleError::caused_by(io_error);
    assert!(error.to_string().contains("file not found"));
}

#[test]
fn test_from_infallible_exists() {
    use std::convert::Infallible;
    fn accepts_from_infallible(_f: fn(Infallible) -> SimpleError) {}
    accepts_from_infallible(SimpleError::from);
}
