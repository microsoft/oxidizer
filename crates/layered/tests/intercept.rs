// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "This is a test module")]
#![cfg(feature = "intercept")]

//! Integration tests for [`Intercept`] middleware.

use layered::{Execute, Intercept, Service, Stack};

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn str_references() {
    let stack = (
        Intercept::<&str, &str, _>::layer()
            .on_input(|input: &&str| {
                assert!(!input.is_empty());
            })
            .on_output(|output: &&str| {
                assert!(!output.is_empty());
            }),
        Execute::new(|input: &str| async move { input }),
    );
    let service = stack.into_service();

    let input = "hello".to_string();
    let output = service.execute(input.as_str()).await;

    assert_eq!(output, "hello");
}
