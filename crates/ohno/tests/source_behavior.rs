// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test to demonstrate the correct source behavior for display override

#![cfg(feature = "test-util")]

use std::error::Error;

use ohno::{Error, OhnoCore, assert_error_message};

#[derive(Error)]
#[display("Custom message")]
struct TestError {
    #[error]
    inner_error: OhnoCore,
}

#[test]
fn test_source_behavior() {
    // Case 1: OhnoCore with string message - source should be None
    let error1 = TestError {
        inner_error: OhnoCore::from("test message"),
    };
    assert!(error1.source().is_none(), "String-based OhnoCore should have no source");

    // Case 2: OhnoCore with wrapped error - source should be Some
    let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file.txt");
    let error2 = TestError {
        inner_error: OhnoCore::from(io_error),
    };
    assert!(error2.source().is_some(), "Error-wrapping OhnoCore should have a source");

    // Verify we get the original io::Error back
    if let Some(source) = error2.source() {
        assert_eq!(format!("{source}"), "file.txt");
    }

    // Case 3: OhnoCore with no source - source should be None
    let error3 = TestError {
        inner_error: OhnoCore::default(),
    };
    assert!(error3.source().is_none(), "Empty OhnoCore should have no source");

    // Case 4: Verify display works correctly
    let display1 = format!("{error1}");
    assert!(display1.contains("Custom message"));
    assert!(display1.contains("caused by: test message"));

    let display2 = format!("{error2}");
    assert!(display2.contains("Custom message"));
    assert!(display2.contains("caused by: file.txt"));

    assert_error_message!(error3, "Custom message");
}
