// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for AppError::root_cause.

use ohno::{AppError, assert_error_message};

#[test]
fn root_cause_simple() {
    let err = AppError::new("simple error");
    let root = err.root_cause();
    assert_error_message!(root.to_string(), "simple error");
    // String errors type is private, so we can't downcast to it
}

#[test]
fn root_cause_with_chain() {
    #[ohno::error]
    struct MiddleError;

    let root_err = std::io::Error::other("disk full");
    let middle_err = MiddleError::caused_by(root_err);
    let err = AppError::new(middle_err);

    let root = err.root_cause();
    assert_error_message!(root.to_string(), "disk full");
    let _ = root.downcast_ref::<std::io::Error>().unwrap();
}

#[test]
fn root_cause_with_deep_chain() {
    #[ohno::error]
    struct Layer1Error;

    #[ohno::error]
    struct Layer2Error;

    #[ohno::error]
    struct Layer3Error;

    let layer3 = Layer3Error::caused_by("network unreachable");
    let layer2 = Layer2Error::caused_by(layer3);
    let layer1 = Layer1Error::caused_by(layer2);
    let err = AppError::new(layer1);

    let root = err.root_cause();
    assert_error_message!(root.to_string(), "network unreachable");
    let _ = root.downcast_ref::<Layer3Error>().unwrap();
}
