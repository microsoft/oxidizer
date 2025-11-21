// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates error trace extension methods for adding traces to Results.

use ohno::ErrorSpanExt;

#[ohno::error]
struct MyError;

fn failing_function() -> Result<String, MyError> {
    Err(MyError::caused_by("connection timeout")).error_span("failed to query database")
}

fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
