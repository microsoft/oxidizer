// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates #[from] attribute with multiple field expressions.

#[derive(Debug, PartialEq, Eq, Default)]
pub enum ErrorKind {
    #[default]
    Unknown,
    Io,
}

#[ohno::error]
#[from(std::io::Error(kind: ErrorKind::Io, message: "IO failed".to_string()))]
pub struct MyError {
    kind: ErrorKind,
    message: String,
}

fn failing_function() -> Result<(), std::io::Error> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found"))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e: MyError = failing_function().unwrap_err().into();
    println!("Kind: {:?}, Message: {:?}", e.kind, e.message);
    println!("Error: {e}");
}
