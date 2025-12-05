// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(missing_docs, reason = "Test code")]

#[fundle::bundle]
struct AppState1 {}

#[fundle::bundle]
struct AppState2 {}

#[test]
fn file_compiles() {}
