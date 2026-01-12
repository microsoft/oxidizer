// Copyright (c) Microsoft Corporation.

//! Demonstrates early returns using the bail! macro.

use ohno::app::AppError;
use ohno::bail;

fn validate_age(age: i32) -> Result<(), AppError> {
    if age < 0 {
        bail!("age cannot be negative: {age}");
    }
    println!("Age {age} is valid");
    Ok(())
}

fn main() {
    validate_age(25).unwrap();

    let err1 = validate_age(-5).unwrap_err();
    println!("\nError: {err1}");
}
