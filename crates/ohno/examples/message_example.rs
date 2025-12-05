// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Example code")]

// Example demonstrating the `message()` method on Ohno error types

use ohno::ErrorExt;

#[ohno::error]
#[display("Failed to process item {item_id} with code {error_code}")]
struct ProcessingError {
    item_id: u32,
    error_code: i32,
}

#[ohno::enrich_err("processing failed")]
fn process(item_id: u32) -> Result<(), ProcessingError> {
    Err(ProcessingError::new(item_id, -456))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let processing_err = process(123).unwrap_err();
    println!("Short message:\n{}", processing_err.message());
    println!("\nFull display output:\n{processing_err}");
}
