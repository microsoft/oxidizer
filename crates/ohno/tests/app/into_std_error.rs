// Copyright (c) Microsoft Corporation.

//! Tests for AppError::into_std_error method.

use ohno::{app::AppError, assert_error_message};

#[test]
fn test_into_std_error_preserves_message() {
    let app_error = AppError::new("test error message");
    assert_eq!(app_error.message(), "test error message");

    let boxed = app_error.into_std_error();
    assert_error_message!(boxed, "test error message");
    assert!(boxed.source().is_none());
}

#[test]
fn test_into_std_error_from_io_error() {
    let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let app_error = AppError::new(io_error);
    assert_error_message!(app_error, "file not found");

    let boxed = app_error.into_std_error();
    assert_error_message!(boxed, "file not found");

    let source = boxed.source().unwrap();
    assert_error_message!(source, "file not found");
    let io_error = source.downcast_ref::<std::io::Error>().unwrap();
    assert_eq!(io_error.kind(), std::io::ErrorKind::NotFound);
}
