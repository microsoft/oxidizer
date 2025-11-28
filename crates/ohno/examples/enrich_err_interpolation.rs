// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating `enrich_err` macro with argument interpolation.

#![expect(clippy::unwrap_used, reason = "Example code")]

use ohno::enrich_err;

#[ohno::error]
struct MyError;

#[enrich_err("failed to load config from '{path}'")]
fn failing_function(path: &str) -> Result<String, MyError> {
    Err(MyError::caused_by("file not found"))
}

fn main() {
    let e = failing_function("config.toml").unwrap_err();
    println!("Error: {e}");
}
