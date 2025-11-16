// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates default constructor generation in #[derive(Error)].

use ohno::{Error, OhnoCore};

#[derive(Error)]
struct MyError {
    path: String,
    inner: OhnoCore,
}

fn main() {
    let _error = MyError::new("/etc/config");
    let error_with_cause = MyError::caused_by("/etc/config", "File not found");
    println!("Error: {error_with_cause}");
}
