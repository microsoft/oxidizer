// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "This is a test module")]
#![cfg(not(miri))]

//! Integration tests for [`Execute`] service.

use layered::{Execute, Service};

#[tokio::test]
async fn str_references() {
    let service = Execute::new(|input: &str| async move { input });

    let output = service.execute("hello").await;

    assert_eq!(output, "hello");
}
