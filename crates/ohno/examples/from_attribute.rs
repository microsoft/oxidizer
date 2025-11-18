// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates using the #[from] attribute to generate From implementations.

#[ohno::error]
#[from(std::io::Error, std::fmt::Error)]
struct MyError {
    optional_field: Option<String>,
    count: u32,
}

fn main() {
    // Test From<std::io::Error>
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let my_err: MyError = io_err.into();
    println!("From io::Error: {my_err}");

    // Test From<std::fmt::Error>
    let fmt_err = std::fmt::Error;
    let my_err: MyError = fmt_err.into();
    println!("From fmt::Error: {my_err}");
}
