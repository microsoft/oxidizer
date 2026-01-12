// Copyright (c) Microsoft Corporation.

//! Tests for the `welp!` macro.

use ohno::assert_error_message;
use ohno::welp;

#[test]
fn welp_with_string() {
    let err = welp!("authentication failed");
    assert_error_message!(err, "authentication failed");
    assert!(err.source().is_none());
}

#[test]
fn welp_with_format() {
    let user_id = 42;
    let err = welp!("user {} not found", user_id);
    assert_error_message!(err, "user 42 not found");
    assert!(err.source().is_none());
}

#[test]
fn welp_with_inline_format() {
    let x = 123;
    let err = welp!("value is {x}");
    assert_error_message!(err, "value is 123");
    assert!(err.source().is_none());
}

#[test]
fn wrap_io_error_with_welp() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "secret.txt");
    let wrapped = welp!(io_err);
    assert_error_message!(wrapped, "secret.txt");
    let _ = wrapped
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .unwrap();
}
