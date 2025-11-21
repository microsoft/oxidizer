// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `ErrorExt::find_source` method

#![cfg(feature = "test-util")]

use ohno::{ErrorExt, assert_error_message};

#[ohno::error]
struct RootCauseError;

#[ohno::error]
struct MiddleError;

#[ohno::error]
struct TopLevelError;

#[ohno::error]
struct PositionalError {
    position: i32,
}

#[test]
fn test_find_source_single_level() {
    let error = TopLevelError::new();

    assert!(error.find_source::<TopLevelError>().is_none());
    assert!(error.find_source::<MiddleError>().is_none());
    assert!(error.find_source::<RootCauseError>().is_none());
}

#[test]
fn test_find_source_two_level_chain() {
    let root_error = RootCauseError::new();
    let top_error = TopLevelError::caused_by(root_error);

    assert!(top_error.find_source::<RootCauseError>().is_some());
    assert!(top_error.find_source::<TopLevelError>().is_none());
    assert!(top_error.find_source::<MiddleError>().is_none());
}

#[test]
fn test_find_source_three_level_chain() {
    let root_error = RootCauseError::new();
    let middle_error = MiddleError::caused_by(root_error);
    let top_error = TopLevelError::caused_by(middle_error);

    assert!(top_error.find_source::<RootCauseError>().is_some());
    assert!(top_error.find_source::<MiddleError>().is_some());
    assert!(top_error.find_source::<TopLevelError>().is_none());
}

#[test]
fn test_find_source_returns_first_match() {
    let first_root = RootCauseError::new();
    let middle_error = MiddleError::caused_by(first_root);
    let second_root = RootCauseError::caused_by(middle_error);
    let top_error = TopLevelError::caused_by(second_root);

    let found = top_error.find_source::<RootCauseError>().unwrap();
    assert_error_message!(found, "RootCauseError");
}

#[test]
fn test_find_source_none_when_no_source() {
    let error = TopLevelError::new();

    assert!(error.find_source::<RootCauseError>().is_none());
    assert!(error.find_source::<MiddleError>().is_none());
}

#[test]
fn test_find_source_with_std_error() {
    let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let root_error = RootCauseError::caused_by(io_error);
    let top_error = TopLevelError::caused_by(root_error);

    assert!(top_error.find_source::<RootCauseError>().is_some());
    let io_error = top_error.find_source::<std::io::Error>().unwrap();
    assert_eq!(io_error.kind(), std::io::ErrorKind::NotFound);
    assert!(top_error.find_source::<MiddleError>().is_none());
}

#[test]
fn test_find_source_deep_nesting() {
    let root = RootCauseError::new();

    let mut current_error: Box<dyn std::error::Error + Send + Sync> = Box::new(root);
    for i in 1..=8 {
        let middle = PositionalError::caused_by(i, current_error);
        current_error = Box::new(middle);
    }

    let top_error = TopLevelError::caused_by(current_error);

    assert!(top_error.find_source::<RootCauseError>().is_some());
    let pos_err_8 = top_error.find_source::<PositionalError>().unwrap();
    assert_eq!(pos_err_8.position, 8);
    let pos_err_7 = pos_err_8.find_source::<PositionalError>().unwrap();
    assert_eq!(pos_err_7.position, 7);
}
