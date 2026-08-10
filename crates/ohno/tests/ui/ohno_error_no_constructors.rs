// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]` adds the `OhnoCore` field itself, and the name of that field is not stable, so
//! a hand-written constructor cannot initialize it.
//!
//! `#[derive(ohno::Error)]` with an explicitly declared core is the supported way to write
//! constructors by hand.

#[ohno::error]
#[no_constructors]
pub struct NoConstructors {
    pub path: String,
}

fn main() {}
