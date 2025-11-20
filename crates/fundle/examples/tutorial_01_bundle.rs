// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_debug_implementations, clippy::empty_structs_with_brackets, reason = "Unit tests")]

// Assume these are random dependencies we want to create and inject.
pub struct Logger {}
pub struct Database {}

// Our "DI" container. Adding `fundle::bundle` adds a `::builder()` method
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
