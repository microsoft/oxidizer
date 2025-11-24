// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating the `#[error_trace]` macro for automatic error trace injection.
//!
//! Shows how the `error_trace` macro adds traces to any errors returned from functions.

use ohno::{Error, OhnoCore, error_trace};

#[derive(Error)]
struct MyError {
    inner: OhnoCore,
}

#[error_trace("failed to load configuration")]
fn failing_function() -> Result<String, MyError> {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "config.toml not found");
    Err(MyError::caused_by(io_err))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
