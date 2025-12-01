// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates error enrichment extension methods for adding traces to Results.

#![expect(clippy::unwrap_used, reason = "Example code")]

use ohno::EnrichableExt;

#[ohno::error]
struct MyError;

fn failing_function() -> Result<String, MyError> {
    Err(MyError::caused_by("connection timeout")).enrich("failed to query database")
}

fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
