// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates #[from] attribute with tuple struct field expressions.

use ohno::OhnoCore;

#[derive(Debug, PartialEq, Eq, Default)]
pub enum ErrorKind {
    #[default]
    Unknown,
    Io,
}

#[derive(ohno::Error)]
#[from(std::io::Error(0: ErrorKind::Io))]
pub struct MyError(ErrorKind, OhnoCore);

fn failing_function() -> Result<(), std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"))
}

fn main() {
    let e: MyError = failing_function().unwrap_err().into();
    println!("Error kind: {:?}", e.0);
    println!("Error: {e}");
}
