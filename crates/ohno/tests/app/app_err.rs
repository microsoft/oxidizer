// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the `app_err!` macro.

use ohno::app_err;
use ohno::assert_error_message;

#[test]
fn app_err_with_string() {
    let err = app_err!("authentication failed");
    assert_error_message!(err, "authentication failed");
    assert!(err.source().is_none());
}

#[test]
fn app_err_with_format() {
    let user_id = 42;
    let err = app_err!("user {} not found", user_id);
    assert_error_message!(err, "user 42 not found");
    assert!(err.source().is_none());
}

#[test]
fn app_err_with_inline_format() {
    let x = 123;
    let err = app_err!("value is {x}");
    assert_error_message!(err, "value is 123");
    assert!(err.source().is_none());
}

#[test]
fn wrap_io_error_with_app_err() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secret.txt");
    let wrapped = app_err!(io_err);
    assert_error_message!(wrapped, "secret.txt");
    let _ = wrapped.source().unwrap().downcast_ref::<std::io::Error>().unwrap();
}
