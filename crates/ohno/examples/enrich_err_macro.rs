// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating the `#[enrich_err]` macro for automatic error trace injection.

#![expect(clippy::unwrap_used, reason = "Example code")]

use ohno::{Error, OhnoCore, enrich_err};

#[derive(Error)]
struct MyError {
    inner: OhnoCore,
}

#[enrich_err("failed to load configuration")]
fn failing_function() -> Result<String, MyError> {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "config.toml not found");
    Err(MyError::caused_by(io_err))
}

fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
