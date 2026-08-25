// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tuple fields are referenced by index, but the index of the `OhnoCore` appended by
//! `#[ohno::error]` is not the user's to reference: `{1}` here would print the error's own chain.

// Index 0 is the declared field, so this type is expected to compile.
#[ohno::error]
#[display("bad path: {0}")]
pub struct DeclaredIndexError(pub String);

// Index 1 holds the injected OhnoCore.
#[ohno::error]
#[display("bad path: {1}")]
pub struct InjectedIndexError(pub String);

fn main() {}
