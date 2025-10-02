#![expect(missing_debug_implementations, clippy::empty_structs_with_brackets, clippy::must_use_candidate, reason = "Unit tests")]

// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Assume these are random dependencies we want to create and inject.
pub struct Logger {}
pub struct Database {}

impl Database {
    pub fn new(_: impl AsRef<Logger>) -> Self {
        Self {}
    }
}

#[fundle::bundle]
pub struct AppState {
    // We can have multiple dependencies of the same type.
    logger_1: Logger,
    logger_2: Logger,
    database: Database,
}

fn main() {
    let _ = AppState::builder()
        .logger_1(|_| Logger {})
        .logger_2(|_| Logger {})
        // However, we now have to resolve the right one via the `AppState!` macro. Basic syntax is
        //
        // let foo = AppState!(select(NAME_OF_X) => TYPE1(FIELD1), TYPE2(FIELD2), ...)
        //
        .database(|x| Database::new(AppState!(select(x) => Logger(logger_1))))
        .build();
}
