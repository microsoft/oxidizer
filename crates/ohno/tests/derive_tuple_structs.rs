// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test for derive Error support on tuple structs

use std::error::Error as StdError;

use ohno::{Error, OhnoCore, assert_error_message};

#[derive(Error)]
struct SimpleTupleError(#[error] OhnoCore);

#[derive(Error)]
struct TupleErrorWithFields(String, i32, #[error] OhnoCore);

#[derive(Error)]
struct TupleErrorAutoDetect(OhnoCore);

#[derive(Error)]
#[from(std::io::Error, std::fmt::Error)]
struct TupleErrorWithFrom(#[error] OhnoCore);

#[test]
fn test_simple_tuple_error() {
    let error = SimpleTupleError(OhnoCore::from("test error"));
    assert!(error.to_string().contains("test error"));
}

#[test]
fn test_tuple_error_with_fields() {
    let error = TupleErrorWithFields("operation".to_string(), 42, OhnoCore::from("failed"));

    // Access the fields to verify they work
    assert_eq!(error.0, "operation");
    assert_eq!(error.1, 42);
    assert!(error.to_string().contains("failed"));
}

#[test]
fn test_tuple_error_auto_detect() {
    let error = TupleErrorAutoDetect(OhnoCore::from("auto detected"));
    assert!(error.to_string().contains("auto detected"));
}

#[test]
fn test_tuple_error_source() {
    let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let error = SimpleTupleError(OhnoCore::from(io_error));

    assert!(error.source().is_some());
}

#[test]
fn test_tuple_error_constructors() {
    // Test new() method
    let error1 = SimpleTupleError::new();
    assert!(error1.to_string().contains("SimpleTupleError"));

    // Test caused_by() method
    let error2 = SimpleTupleError::caused_by("test error");
    assert!(error2.to_string().contains("test error"));
}

#[test]
fn test_tuple_error_with_fields_constructors() {
    // Test new() method with multiple fields
    let error1 = TupleErrorWithFields::new("operation", 42);
    assert!(error1.to_string().contains("TupleErrorWithFields"));

    // Test caused_by() method with multiple fields
    let error2 = TupleErrorWithFields::caused_by("operation", 42, "custom error");
    assert!(error2.to_string().contains("custom error"));
}

#[test]
fn test_tuple_error_span() {
    use ohno::ErrorSpan;

    let mut error = SimpleTupleError(OhnoCore::from("test"));
    error.add_error_span(ohno::SpanInfo::new("span message"));

    // The error should still be valid after adding span
    assert!(error.to_string().contains("test"));
}

#[test]
fn test_tuple_error_from_implementations() {
    use std::io;

    // Test From<std::io::Error>
    let io_error = io::Error::new(io::ErrorKind::NotFound, "file not found");
    let error: TupleErrorWithFrom = io_error.into();
    assert_error_message!(error, "file not found");

    // Test From<std::fmt::Error>
    let fmt_error = std::fmt::Error;
    let error: TupleErrorWithFrom = fmt_error.into();
    println!("Error: {error}");
    assert_error_message!(error, "an error occurred when formatting an argument");
}
