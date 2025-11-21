// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating `error_span` macro with argument interpolation.
//!
//! Shows how the `error_span` macro can interpolate function arguments into the trace message.

use ohno::error_span;

#[ohno::error]
struct MyError;

#[error_span("failed to load config from '{path}'")]
fn failing_function(path: &str) -> Result<String, MyError> {
    Err(MyError::caused_by("file not found"))
}

fn main() {
    let e = failing_function("config.toml").unwrap_err();
    println!("Error: {e}");
}
