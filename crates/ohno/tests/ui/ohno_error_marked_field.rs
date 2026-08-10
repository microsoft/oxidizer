// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]` always adds the `OhnoCore` field and always generates the error representation
//! from it, so a marker on another field asks for something the attribute cannot honor.
//!
//! Declaring a field of type `OhnoCore` is fine: the injected field is the marked one, so the
//! declared field stays an ordinary field.

#[ohno::error]
pub struct AlreadyMarked {
    pub path: String,
    #[error]
    inner: ohno::OhnoCore,
}

fn main() {}
