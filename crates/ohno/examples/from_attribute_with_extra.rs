// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates multiple #[from] attributes.

#[derive(Debug, PartialEq, Eq, Default)]
pub enum ErrorKind {
    #[default]
    Unknown,
    Io,
    Format,
}

#[ohno::error]
#[from(std::io::Error(kind: ErrorKind::Io))]
#[from(std::fmt::Error(kind: ErrorKind::Format))]
pub struct MyError {
    kind: ErrorKind,
}

fn failing_function() -> Result<(), MyError> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "file not found").into())
}

fn main() {
    let e = failing_function().unwrap_err();
    println!("Error kind: {:?}", e.kind);
}
