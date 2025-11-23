// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Example showing multiple `OhnoCore` fields requiring `#[error]` attribute.

use ohno::{Error, OhnoCore};

#[derive(Error)]
struct MyError {
    context_info: OhnoCore,
    #[error] // Mark the primary error field
    main_error: OhnoCore,
}

fn failing_function() -> Result<(), MyError> {
    Err(MyError {
        context_info: OhnoCore::from("Additional context"),
        main_error: OhnoCore::from("Main error occurred"),
    })
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_function().unwrap_err();
    println!("Error: {e}");
}
