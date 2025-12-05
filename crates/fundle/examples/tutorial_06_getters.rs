// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    missing_debug_implementations,
    clippy::empty_structs_with_brackets,
    clippy::must_use_candidate,
    missing_docs,
    reason = "Unit tests"
)]

pub struct Logger {}
pub struct Database {}

impl Database {
    // Some dependency asked for by normal reference
    pub const fn new(_: &Logger) -> Self {
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
        .database(|x| Database::new(x.logger()))
        .build();
}
