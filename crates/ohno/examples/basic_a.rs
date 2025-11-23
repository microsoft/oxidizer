// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates using #[`ohno::error`] to create a simple error type.

#[ohno::error]
struct MyError;

fn failing_function() -> Result<String, MyError> {
    Err(MyError::caused_by("custom message"))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
