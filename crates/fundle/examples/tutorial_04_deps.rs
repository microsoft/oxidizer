#![expect(missing_debug_implementations, clippy::empty_structs_with_brackets, reason = "Unit tests")]
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![expect(dead_code, reason = "This is just an example.")]

// Assume these are random dependencies we want to inject.
#[derive(Clone)]
pub struct Logger {}

// You can declare your 'import' dependencies via `fundle::deps`.
#[fundle::deps]
struct DatabaseDeps {
    _logger: Logger,
}

pub struct Database {}

impl Database {
    // And then ask for them via `impl Into<MyDeps>`.s
    fn new(_: impl Into<DatabaseDeps>) -> Self {
        Self {}
    }
}

fn main() {}
