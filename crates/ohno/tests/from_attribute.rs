// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]
#![cfg(feature = "test-util")]

use ohno::{Error, OhnoCore, assert_error_message};

#[test]
fn test_from_attribute_single_type() {
    #[derive(Error, Default)]
    #[from(std::io::Error)]
    struct MyError {
        inner: OhnoCore,
        code: u32,
    }

    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test error");
    let my_err: MyError = io_err.into();

    // Verify the error field is set correctly
    assert_error_message!(my_err, "test error");

    // Verify other fields are defaulted
    assert_eq!(my_err.code, 0);
}

#[test]
fn test_from_attribute_multiple_types() {
    #[derive(Error, Default)]
    #[from(std::io::Error, std::fmt::Error)]
    struct MultiError {
        inner: OhnoCore,
        optional_field: Option<String>,
        count: usize,
    }

    // Test From<std::io::Error>
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "io error");
    let multi_err: MultiError = io_err.into();
    assert_error_message!(multi_err, "io error");
    assert_eq!(multi_err.optional_field, None);
    assert_eq!(multi_err.count, 0);

    // Test From<std::fmt::Error>
    let fmt_err = std::fmt::Error;
    let multi_err: MultiError = fmt_err.into();
    assert_error_message!(multi_err, "an error occurred when formatting an argument");
    assert_eq!(multi_err.optional_field, None);
    assert_eq!(multi_err.count, 0);
}

#[test]
fn test_from_attribute_complex_fields() {
    #[derive(Error, Default)]
    #[from(std::io::Error)]
    struct ComplexError {
        inner: OhnoCore,
        data: Vec<u8>,
        flags: bool,
        info: Option<String>,
    }

    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let complex_err: ComplexError = io_err.into();

    assert_error_message!(complex_err, "access denied");
    assert!(complex_err.data.is_empty());
    assert!(!complex_err.flags);
    assert!(complex_err.info.is_none());
}

#[test]
fn test_from_attribute_with_custom_error_field() {
    #[derive(Error, Default)]
    #[from(std::io::Error)]
    struct CustomFieldError {
        #[error]
        error_core: OhnoCore,
        metadata: String,
    }

    let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
    let custom_err: CustomFieldError = io_err.into();

    assert_error_message!(custom_err, "timeout");
    assert!(custom_err.metadata.is_empty());
}
