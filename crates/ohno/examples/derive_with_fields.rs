// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prints `InvalidQuery`

use ohno::{Error, OhnoCore};

#[derive(Error)]
struct InvalidQuery {
    operation: String,
    table: String,
    inner: OhnoCore,
}

fn failing_query() -> Result<String, InvalidQuery> {
    Err(InvalidQuery::new("SELECT", "users"))
}

fn main() {
    let e = failing_query().unwrap_err();
    println!("{e}");
    println!("{e:#?}");
}
