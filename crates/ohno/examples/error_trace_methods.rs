// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates error trace extension methods for adding traces to Results.

use ohno::ErrorTraceExt;

#[ohno::error]
struct MyError;

fn failing_function() -> Result<String, MyError> {
    Err(MyError::caused_by("connection timeout")).error_trace("failed to query database")
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
