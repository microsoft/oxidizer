// Copyright (c) Microsoft Corporation.

//! Example demonstrating how to transform `AppError` into a standard error trait object.

#![expect(clippy::unwrap_used, reason = "example code")]

use ohno::app::AppError;

#[ohno::error]
struct MyLibError;

impl From<AppError> for MyLibError {
    fn from(value: AppError) -> Self {
        Self::caused_by(value.into_std_error())
    }
}

/// Simulates a function that returns an `AppError`.
fn database_operation() -> Result<(), AppError> {
    Err(AppError::new("connection timeout after 30s"))
}

/// Simulates a library function that uses its own error type
fn service_operation() -> Result<(), MyLibError> {
    database_operation()?;
    Ok(())
}

fn main() {
    let err = service_operation().unwrap_err();
    println!("{err}");
}
