// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An example demonstrating a clonable error using the `ohno` crate.

#[ohno::error]
#[derive(Clone)]
#[display("ClonableError: str_field={str_field}, int_field={int_field}")]
struct ClonableError {
    str_field: String,
    int_field: i32,
}

fn main() {
    let io_err = std::io::Error::other("I/O failure");
    let err = ClonableError::caused_by("example string", 42, io_err);
    let cloned_err = err.clone();

    println!("Original Error: {err}",);
    println!("Cloned Error: {cloned_err}");
}
