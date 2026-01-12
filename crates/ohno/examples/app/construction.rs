// Copyright (c) Microsoft Corporation.

//! Demonstrates simple error construction with welp! and Error::new().

use ohno::app::AppError;
use ohno::welp;

fn main() {
    let err = AppError::new("database connection failed");
    println!("Error::new(): {err}\n");

    // Using welp! with format arguments
    let user_id = 42;
    let err = welp!("user {user_id} not found");
    println!("welp! with format: {err}");

    // Using welp! with a IO error as the source
    let err = welp!(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "file missing"
    ));
    println!("welp! with source: {err}");
}
