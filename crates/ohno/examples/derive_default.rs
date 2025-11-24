// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates Default behavior with #[derive(Error)].

use ohno::{Error, OhnoCore};

#[derive(Error, Default)]
struct MyError {
    inner: OhnoCore,
    optional_field: Option<String>,
}

fn failing_function() -> Result<(), MyError> {
    Err(MyError::default())
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
    println!("Optional field: {:?}", e.optional_field);
}
