// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates automatic conversion of errors using the ? operator.

#![expect(clippy::unwrap_used, reason = "example code")]

use ohno::AppError;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};

fn validate_and_process(value: i32) -> Result<i32, AppError> {
    if value < 0 {
        Err(IoError::new(
            IoErrorKind::InvalidInput,
            format!("value cannot be negative: {value}"),
        ))?;
    }

    Ok(value * 2)
}

fn main() {
    // Valid value
    let result = validate_and_process(50).unwrap();
    println!("Success: {result}\n");

    // Negative value error
    let err = validate_and_process(-5).unwrap_err();
    println!("Error: {err}");
}
