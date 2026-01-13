// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for bail! macro for early returns.

use ohno::app::AppError;
use ohno::assert_error_message;
use ohno::bail;

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
