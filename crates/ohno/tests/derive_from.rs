// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};

use ohno::{OhnoCore, assert_error_message};

#[derive(ohno::Error)]
#[from(std::io::Error(kind: error.kind()))]
struct TestError {
    kind: IoErrorKind,
    core: OhnoCore,
}

#[derive(ohno::Error)]
#[from(std::io::Error(0: error.kind()))]
struct TestTupleError(IoErrorKind, OhnoCore);

#[derive(ohno::Error)]
#[from(std::io::Error(kind: error.kind()))]
#[from(std::fmt::Error(kind: IoErrorKind::Other))]
struct TestDblError {
    kind: IoErrorKind,
    core: OhnoCore,
}

#[test]
fn test_from_io_error_kind() {
    let io_err = IoError::new(IoErrorKind::NotFound, "file not found");
    let custom_err: TestError = io_err.into();
    assert_eq!(custom_err.kind, IoErrorKind::NotFound);
    assert_error_message!(custom_err, "file not found");
    assert!(custom_err.source().is_some());
}

#[test]
fn test_from_io_error_other_kind() {
    let io_err = IoError::new(IoErrorKind::PermissionDenied, "access denied");
    let custom_err: TestError = io_err.into();
    assert_eq!(custom_err.kind, IoErrorKind::PermissionDenied);
    assert_error_message!(custom_err, "access denied");
    assert!(custom_err.source().is_some());
}

#[test]
fn test_from_io_error_empty_message() {
    let io_err = IoError::other("other error");
    let custom_err: TestError = io_err.into();
    assert_eq!(custom_err.kind, IoErrorKind::Other);
    assert_error_message!(custom_err, "other error");
    assert!(custom_err.source().is_some());
}

#[test]
fn test_from_io_error_tuple_type() {
    let io_err = IoError::new(IoErrorKind::TimedOut, "timeout occurred");
    let custom_err: TestTupleError = io_err.into();
    assert_eq!(custom_err.0, IoErrorKind::TimedOut);
    assert_error_message!(custom_err, "timeout occurred");
    assert!(custom_err.source().is_some());
}

#[test]
fn test_from_io_error_dbl_error() {
    let io_err = IoError::new(IoErrorKind::NotConnected, "not connected");
    let custom_err: TestDblError = io_err.into();
    assert_eq!(custom_err.kind, IoErrorKind::NotConnected);
    assert_error_message!(custom_err, "not connected");
    assert!(custom_err.source().is_some());
}

#[test]
fn test_from_fmt_error_dbl_error() {
    let fmt_err = std::fmt::Error;
    assert_error_message!(fmt_err, "an error occurred when formatting an argument");
    let custom_err: TestDblError = fmt_err.into();
    assert_eq!(custom_err.kind, IoErrorKind::Other);
    assert_error_message!(custom_err, "an error occurred when formatting an argument");
    assert!(custom_err.source().is_some());
}
