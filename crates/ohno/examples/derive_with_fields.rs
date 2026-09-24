// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prints an `InvalidQuery` through both `Display` and `Debug`.

use ohno::{Error, OhnoCore};

#[derive(Error)]
#[display("invalid query: {operation} on {table}")]
struct InvalidQuery {
    operation: String,
    table: String,
    inner: OhnoCore,
}

fn failing_query() -> Result<String, InvalidQuery> {
    Err(InvalidQuery::new("SELECT", "users"))
}

#[expect(clippy::unwrap_used, reason = "Example code")]
fn main() {
    let e = failing_query().unwrap_err();
    println!("{e}");
    println!("{e:#?}");
}
