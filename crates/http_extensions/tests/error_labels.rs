// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The labels that `collect_error_labels` returns for error chains.

use std::io::{self, ErrorKind};

use http_extensions::{HttpError, collect_error_labels};
use recoverable::RecoveryInfo;

#[derive(Debug)]
struct DummyError;

impl std::fmt::Display for DummyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dummy")
    }
}

impl std::error::Error for DummyError {}

#[test]
fn resolves_http_error_label() {
    let error = HttpError::other("boom", RecoveryInfo::never(), "scheme_not_allowed");

    assert_eq!(collect_error_labels(&error), "scheme_not_allowed");
}

#[test]
fn resolves_io_error_kind_label() {
    let error = io::Error::from(ErrorKind::ConnectionRefused);

    assert_eq!(collect_error_labels(&error), "connection_refused");
}

#[test]
fn resolves_io_error_with_unrelated_inner_uses_outer_kind() {
    let error = io::Error::new(ErrorKind::PermissionDenied, DummyError);

    assert_eq!(collect_error_labels(&error), "permission_denied");
}

#[test]
fn unwraps_nested_io_error() {
    let error = io::Error::other(io::Error::from(ErrorKind::ConnectionReset));

    assert_eq!(collect_error_labels(&error), "connection_reset");
}
