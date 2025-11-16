// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests demonstrating #[derive(Error)] functionality

use ohno::{Error, OhnoCore, assert_error_message};

#[test]
fn test_simple_error_constructors() {
    #[ohno::error]
    struct NetworkError;

    let net_err = NetworkError::new();
    assert_error_message!(net_err, "NetworkError");

    let net_err_with_error = NetworkError::caused_by("Connection timeout");
    assert_error_message!(net_err_with_error, "Connection timeout");
}

#[test]
fn test_complex_error_constructors() {
    #[ohno::error]
    struct DatabaseError {
        table: String,
        operation: String,
    }

    let db_err = DatabaseError::new("users", "SELECT");
    assert_error_message!(db_err, "DatabaseError");

    let db_err_with_error = DatabaseError::caused_by("users", "SELECT", "Table not found");
    assert_error_message!(db_err_with_error, "Table not found");
}

#[test]
fn test_derive_error_constructors() {
    #[derive(Error)]
    struct TestError {
        field: String,
        inner: OhnoCore,
    }

    // Test that #[derive(Error)] constructors work by default
    let val_err = TestError::new("email");
    assert_eq!(val_err.field, "email");

    let val_err_with_error = TestError::caused_by("email", "Invalid format");
    assert_eq!(val_err_with_error.field, "email");
    assert_error_message!(val_err_with_error, "Invalid format");
}
