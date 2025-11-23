// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This is a comparison of anyhow+thiserror error handling with ohno error handling.
//! Ohno example: crates/ohno/examples/chained.rs

use anyhow::{Context, Result};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("Database error (1)")]
struct DatabaseError {
    #[from]
    source: std::io::Error,
}

#[derive(Debug, Error)]
#[error("Service error (2)")]
struct ServiceError {
    #[from]
    source: DatabaseError,
}

#[derive(Debug, Error)]
#[error("API error (3)")]
struct ApiError {
    #[from]
    source: ServiceError,
}

fn database_operation() -> Result<String, DatabaseError> {
    Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied (0)").into())
}

fn service_operation() -> Result<String, ServiceError> {
    Ok(database_operation()?)
}

fn api_operation() -> Result<String, ApiError> {
    Ok(service_operation()?)
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let result: Result<String> = api_operation()
        .context("handling API request (3.1)")
        .context("processing request payload (3.2)")
        .context("preparing response (3.3)");

    let error = result.unwrap_err();
    println!("Display:\n{error}");
    println!("\nDebug:\n{error:?}");
}

/*
Output:

Display:
preparing response (3.3)

Debug:
preparing response (3.3)

Caused by:
    0: processing request payload (3.2)
    1: handling API request (3.1)
    2: API error (3)
    3: Service error (2)
    4: Database error (1)
    5: access denied (0)
*/
