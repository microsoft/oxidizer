#![expect(missing_debug_implementations, clippy::empty_structs_with_brackets, clippy::must_use_candidate, reason = "Unit tests")]

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Assume these are random dependencies we want to create and inject.
pub struct Logger {}
pub struct Database {}

impl Database {
    // Database here now depends on `Logger` to work.
    pub fn new(_: impl AsRef<Logger>) -> Self {
        Self {}
    }
}

#[fundle::bundle]
pub struct AppState {
    logger: Logger,
    database: Database,
}

fn main() {
    let _ = AppState::builder()
        .logger(|_| Logger {})
        // Here we automatically resolve the dependencies of `Database`.
        .database(|x| Database::new(x))
        .build();
}
