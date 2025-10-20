#![expect(
    missing_debug_implementations,
    clippy::empty_structs_with_brackets,
    clippy::must_use_candidate,
    reason = "Unit tests"
)]

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Assume these are random dependencies we want to create and inject.
pub struct Logger {}
pub struct Database {}

// Our "DI" container (and in Oxidizer would be the "thread-per-core"
// app state). Adding `fundle::bundle` adds a `::builder()` method
// to construct this.
#[fundle::bundle]
pub struct AppState {
    logger: Logger,
    database: Database,
}

fn main() {
    // Create a new instance of `AppState`.
    let _ = AppState::builder().logger(|_| Logger {}).database(|_| Database {}).build();
}
