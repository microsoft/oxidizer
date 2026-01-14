// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates simple error construction with `app_err!` and `ohno::AppError::new()`.

use ohno::AppError;
use ohno::app_err;

fn main() {
    let err = AppError::new("database connection failed");
    println!("Error::new(): {err}\n");

    // Using app_err! with format arguments
    let user_id = 42;
    let err = app_err!("user {user_id} not found");
    println!("app_err! with format: {err}");

    // Using app_err! with a IO error as the source
    let err = app_err!(std::io::Error::new(std::io::ErrorKind::NotFound, "file missing"));
    println!("app_err! with source: {err}");
}
