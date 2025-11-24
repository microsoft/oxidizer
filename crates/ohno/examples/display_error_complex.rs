// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prints:
//! Operation 'update' failed with code 404
//! Caused by:
//!         Not found

#[derive(Debug)]
struct ErrorCode(u32);

#[ohno::error]
#[display("Operation '{operation}' failed with code {}", code.0)]
struct MyError {
    operation: String,
    code: ErrorCode,
}

fn failing_function() -> Result<(), MyError> {
    Err(MyError::caused_by("update", ErrorCode(404), "Not found"))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("{e}");
}
