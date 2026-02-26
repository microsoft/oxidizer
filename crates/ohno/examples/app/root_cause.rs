// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates finding the root cause of an error chain.

#![expect(clippy::unwrap_used, reason = "example code")]

use std::io::Error as IoError;

use ohno::AppError;

#[ohno::error]
struct DatabaseError;

#[ohno::error]
struct ConnectionError;

fn main() {
    let io_err = IoError::other("network unreachable");
    let conn_err = ConnectionError::caused_by(io_err);
    let db_err = DatabaseError::caused_by(conn_err);
    let err = AppError::new(db_err);

    let root = err.root_cause();
    let io_err = root.downcast_ref::<IoError>().unwrap();
    println!("Root cause IO error: {io_err}");
}
