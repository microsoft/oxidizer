// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error;

use ohno::{ErrorExt, assert_error_message};

#[ohno::error]
pub struct TestError;

fn error_from_str(s: &str) -> TestError {
    TestError::caused_by(s)
}

#[test]
fn test() {
    let error = {
        let s = String::from("test");
        error_from_str(&s)
    };

    assert_error_message!(error, "test");
    assert_eq!(error.message(), "test");
    assert!(error.source().is_none());

    let debug_str = format!("{error:?}");
    assert!(debug_str.contains("source: Transparent("), "{debug_str}");
}
