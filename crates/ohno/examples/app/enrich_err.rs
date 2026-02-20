// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates adding context to errors using `error_trace`.

#![expect(clippy::unwrap_used, reason = "example code")]

use ohno::{AppError, bail, enrich_err};

#[enrich_err("failed to load data for user {user_id}")]
fn load_user_data(user_id: u32) -> Result<String, AppError> {
    bail!(std::io::Error::new(std::io::ErrorKind::NotFound, "user.db"))
}

fn main() {
    let err = load_user_data(123).unwrap_err();
    println!("Error with context: {err}\n");
}
