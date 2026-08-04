// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[error(generated)]` is written by `#[ohno::error]` onto the `OhnoCore` field it injects, so
//! the macro knows that field's type exactly and rejects the marker on any other type.
//!
//! The bare `#[error]` marker is deliberately not type-checked, because the field may hold an
//! alias or a re-export of `OhnoCore` that cannot be recognized by name. Marking a field that is
//! genuinely not a core type therefore still fails, but against the generated implementations
//! rather than against the attribute — the second case below pins that weaker diagnostic.

#[derive(ohno::Error)]
pub struct GeneratedMarkerOnAnotherType {
    #[error(generated)]
    hidden: String,
    inner: ohno::OhnoCore,
}

#[derive(ohno::Error)]
pub struct BareMarkerOnAnotherType {
    #[error]
    not_a_core: String,
}

fn main() {}
