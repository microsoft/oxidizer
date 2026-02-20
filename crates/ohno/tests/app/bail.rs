// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for bail! macro for early returns.

use ohno::{AppError, assert_error_message, bail};

#[test]
fn bail_with_string() {
    fn fail() -> Result<(), AppError> {
        bail!("operation failed");
    }

    let err = fail().unwrap_err();
    assert_error_message!(err, "operation failed");
    assert!(err.source().is_none());
}

#[test]
fn bail_with_format() {
    fn validate_age(age: i32) -> Result<(), AppError> {
        if age < 0 {
            bail!("age cannot be negative: {}", age);
        }
        Ok(())
    }

    let err = validate_age(-5).unwrap_err();
    assert_error_message!(err, "age cannot be negative: -5");
    assert!(err.source().is_none());
}

#[test]
fn bail_with_interpolation() {
    fn validate_age(age: i32) -> Result<(), AppError> {
        if age < 0 {
            bail!("age cannot be negative: {age}");
        }
        Ok(())
    }

    let err = validate_age(-5).unwrap_err();
    assert_error_message!(err, "age cannot be negative: -5");
    assert!(err.source().is_none());
}

#[test]
fn bail_with_error_ident() {
    fn fail() -> Result<(), AppError> {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "config.toml");
        bail!(io_err);
    }

    let err = fail().unwrap_err();
    assert_error_message!(err, "config.toml");

    let io_err = err.source().unwrap().downcast_ref::<std::io::Error>().unwrap();
    assert!(io_err.kind() == std::io::ErrorKind::NotFound);
}

#[test]
fn bail_with_expr() {
    fn fail() -> Result<(), AppError> {
        bail!(std::io::Error::other("test error"));
    }

    let err = fail().unwrap_err();
    assert_error_message!(err, "test error");

    let io_err = err.source().unwrap().downcast_ref::<std::io::Error>().unwrap();
    assert!(io_err.kind() == std::io::ErrorKind::Other);
}

#[test]
fn downcast_ref() {
    let err = AppError::new(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"));
    assert!(err.downcast_ref::<std::fmt::Error>().is_none());
    let io_err: &std::io::Error = err.downcast_ref().unwrap();
    assert!(io_err.kind() == std::io::ErrorKind::PermissionDenied);

    let err = AppError::new(std::fmt::Error);
    assert!(err.downcast_ref::<std::io::Error>().is_none());
    let _ = err.downcast_ref::<std::fmt::Error>().unwrap();

    let err = AppError::new("a string error");
    assert!(err.downcast_ref::<std::io::Error>().is_none());
    assert!(err.downcast_ref::<std::fmt::Error>().is_none());
}

#[test]
fn into_boxed() {
    let err = AppError::new(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"));
    let boxed: Box<dyn std::error::Error + Send + Sync + 'static> = err.into();
    assert!(boxed.downcast_ref::<std::io::Error>().is_none()); // this is an AppError instance with io::Error inside
    let io_err: &std::io::Error = boxed.source().unwrap().downcast_ref().unwrap();
    assert!(io_err.kind() == std::io::ErrorKind::BrokenPipe);
}
