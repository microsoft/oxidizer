// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An example demonstrating how to convert errors into `AppError` using the `IntoAppError` trait.

#![expect(clippy::unwrap_used, reason = "example code")]

use ohno::app::{AppError, IntoAppError};

fn io_operation() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("simulated I/O error"))
}

fn do_io_operation() -> Result<(), AppError> {
    io_operation().into_app_err("failed to perform I/O operation")
}

fn main() {
    let err: AppError = do_io_operation().unwrap_err();
    println!("{err}");
}
