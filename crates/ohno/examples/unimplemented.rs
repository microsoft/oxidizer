// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "example code")]

use ohno::{Unimplemented, unimplemented_error};

#[ohno::error]
#[from(Unimplemented)]
pub struct MyError;

fn do_something(is_lucky: bool) -> Result<(), MyError> {
    if is_lucky {
        Ok(())
    } else {
        unimplemented_error!("this feature is not yet implemented");
    }
}

fn main() {
    let err = do_something(false).unwrap_err();
    println!("Error: {err}");
}

// Output:
// Error: not implemented at crates\ohno\examples\unimplemented.rs:11
// caused by: this feature is not yet implemented
