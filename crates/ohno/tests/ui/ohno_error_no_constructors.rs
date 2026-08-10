// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]` adds the `OhnoCore` field itself, so opting out of the generated constructors
//! would leave a hand-written constructor naming a field the attribute chose — and renames when the
//! struct later declares an `ohno_core` of its own.
//!
//! `#[derive(ohno::Error)]` with a declared core is the supported way to write constructors by hand.

#[ohno::error]
#[no_constructors]
pub struct NoConstructors {
    pub path: String,
}

fn main() {}
