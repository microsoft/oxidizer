// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Comprehensive test that verifies the new default constructor behavior
//! and the `no_constructors` opt-out functionality

#![cfg(feature = "test-util")]

use ohno::{Error, OhnoCore, assert_error_message};

// These error types should have constructors by default (no attributes needed)
#[derive(Error)]
struct DefaultSimpleError {
    #[error]
    inner: OhnoCore,
}

#[derive(Error)]
struct DefaultComplexError {
    message: String,
    code: u32,
    #[error]
    inner: OhnoCore,
}

// This error type should NOT have constructors (explicit opt-out)
#[derive(Error)]
#[no_constructors]
struct NoConstructorsError {
    #[error]
    inner: OhnoCore,
}

#[test]
fn test_default_simple_constructors() {
    // new() method should be available by default
    let error = DefaultSimpleError::new();
    assert_error_message!(error, "DefaultSimpleError");

    // caused_by() method should be available by default
    let error = DefaultSimpleError::caused_by("test error");
    assert_error_message!(error, "test error");
}

#[test]
fn test_default_complex_constructors() {
    // new() method should work with multiple fields
    let error = DefaultComplexError::new("Processing failed", 404u32);
    assert_eq!(error.message, "Processing failed");
    assert_eq!(error.code, 404);

    // caused_by() method should work with multiple fields
    let error = DefaultComplexError::caused_by("Processing failed", 404u32, "Database connection lost");
    assert_eq!(error.message, "Processing failed");
    assert_eq!(error.code, 404);
    assert!(format!("{error}").contains("Database connection lost"));
}

#[test]
fn test_no_constructors() {
    // The NoConstructorsError should NOT have new() or caused_by() methods
    // This test verifies that we can manually construct the error
    let error = NoConstructorsError {
        inner: OhnoCore::builder().error("manually constructed").build(),
    };
    assert!(format!("{error}").contains("manually constructed"));

    // Note: Attempting to call NoConstructorsError::new() or NoConstructorsError::caused_by()
    // would result in a compile error, which is exactly what we want
}

#[test]
fn test_constructor_compatibility() {
    // Test that all constructor types work with various error sources

    // String source
    let _error1 = DefaultSimpleError::caused_by("string error");

    // std::io::Error source
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let _error3 = DefaultSimpleError::caused_by(io_err);

    // All should compile and work correctly
}
