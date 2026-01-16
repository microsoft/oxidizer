// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the #[backtrace(...)] attribute

#![cfg(feature = "test-util")]

use std::backtrace::BacktraceStatus;

use ohno::ErrorExt;

#[ohno::error]
#[backtrace(force)]
struct ForcedBacktraceError;

#[ohno::error]
#[backtrace(disabled)]
struct DisabledBacktraceError;

#[ohno::error]
struct AutoBacktraceError;

#[ohno::error]
#[backtrace(force)]
struct ForcedBacktraceWithFields {
    message: String,
}

#[ohno::error]
#[backtrace(disabled)]
struct DisabledBacktraceWithFields {
    code: i32,
}

#[ohno::error]
#[backtrace(force)]
#[from(std::io::Error)]
struct ForcedBacktraceWithFrom;

#[test]
fn test_forced_backtrace_always_captures() {
    let error = ForcedBacktraceError::new();
    assert_eq!(error.backtrace().status(), BacktraceStatus::Captured);
}

#[test]
fn test_forced_backtrace_caused_by_captures() {
    let error = ForcedBacktraceError::caused_by("inner error");
    assert_eq!(error.backtrace().status(), BacktraceStatus::Captured);
}

#[test]
fn test_disabled_backtrace_never_captures() {
    let error = DisabledBacktraceError::new();
    assert_eq!(error.backtrace().status(), BacktraceStatus::Disabled);
}

#[test]
fn test_disabled_backtrace_caused_by_never_captures() {
    let error = DisabledBacktraceError::caused_by("inner error");
    assert_eq!(error.backtrace().status(), BacktraceStatus::Disabled);
}

#[test]
fn test_forced_backtrace_with_fields() {
    let error = ForcedBacktraceWithFields::new("test message");
    assert_eq!(error.message, "test message");
    assert_eq!(error.backtrace().status(), BacktraceStatus::Captured);

    let error = ForcedBacktraceWithFields::caused_by("test message", "inner error");
    assert_eq!(error.message, "test message");
    assert_eq!(error.backtrace().status(), BacktraceStatus::Captured);
}

#[test]
fn test_disabled_backtrace_with_fields() {
    let error = DisabledBacktraceWithFields::new(42);
    assert_eq!(error.code, 42);
    assert_eq!(error.backtrace().status(), BacktraceStatus::Disabled);

    let error = DisabledBacktraceWithFields::caused_by(42, "inner error");
    assert_eq!(error.code, 42);
    assert_eq!(error.backtrace().status(), BacktraceStatus::Disabled);
}

#[test]
fn test_forced_backtrace_with_from() {
    let io_error = std::io::Error::other("test io error");
    let error = ForcedBacktraceWithFrom::from(io_error);
    assert_eq!(error.backtrace().status(), BacktraceStatus::Captured);
}

#[test]
fn test_auto_backtrace_respects_environment() {
    // Auto backtrace depends on RUST_BACKTRACE environment variable
    // We can't guarantee the environment in tests, but we can verify
    // that the error is created successfully
    let error = AutoBacktraceError::new();
    // Status could be Captured or Disabled depending on environment
    let status = error.backtrace().status();
    assert!(status == BacktraceStatus::Captured || status == BacktraceStatus::Disabled);
}
