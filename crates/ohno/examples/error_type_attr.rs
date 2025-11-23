// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates #[`ohno::error`] transforming existing structs into error types.

#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub enum ErrorKind {
    #[default]
    Network,
    Database,
}

/// Doc comment
#[ohno::error]
#[from(std::io::Error(kind: ErrorKind::Network, operation: "network_request".to_owned()))]
pub struct AppError {
    pub kind: ErrorKind,
    pub operation: String,
}

fn io_err() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("io_error"))
}

fn app_err() -> Result<(), AppError> {
    io_err()?;
    Ok(())
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = app_err().unwrap_err();
    println!("{e}");
    println!("{e:#?}");
}
