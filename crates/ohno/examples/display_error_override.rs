// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prints `Failed to read file`

use ohno::{Error, OhnoCore};

#[derive(Error, Default)]
#[display("Failed to read file")]
struct MyError {
    inner: OhnoCore,
}

fn failing_function() -> Result<(), MyError> {
    Err(MyError::default())
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("{e}");
}
