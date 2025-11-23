// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates using #[derive(Error)] to create a simple error type.

use ohno::{Error, OhnoCore};

#[derive(Error)]
struct MyError {
    inner: OhnoCore,
}

fn failing_function() -> Result<String, MyError> {
    Err(MyError::caused_by("custom message"))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
