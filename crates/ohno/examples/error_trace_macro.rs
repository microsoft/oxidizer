// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating the `#[error_span]` macro for automatic error span injection.
//!
//! Shows how the `error_span` macro adds spans to ohno-bases errors returned from functions.

use ohno::{Error, OhnoCore, error_span};

#[derive(Error)]
struct MyError {
    inner: OhnoCore,
}

#[error_span("failed to load configuration")]
fn failing_function() -> Result<String, MyError> {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "config.toml not found");
    Err(MyError::caused_by(io_err))
}

fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
