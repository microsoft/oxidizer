// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example demonstrating how to disable default constructors.
//!
//! Shows the `#[no_constructors]` attribute to opt-out of default behavior.

use ohno::{Error, OhnoCore};

#[derive(Error)]
#[no_constructors]
struct MyError {
    inner: OhnoCore,
}

fn failing_function() -> Result<(), MyError> {
    // Must construct manually when constructors are disabled
    Err(MyError {
        inner: OhnoCore::from("Manually constructed error"),
    })
}

fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
