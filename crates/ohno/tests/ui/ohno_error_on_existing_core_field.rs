// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]` adds the `OhnoCore` field itself, so a struct that marks or declares one of
//! its own would end up holding two, with the field the implementations are generated from left
//! to declaration order and the other unused.

#[ohno::error]
pub struct AlreadyMarked {
    pub path: String,
    #[error]
    inner: ohno::OhnoCore,
}

#[ohno::error]
pub struct AlreadyDeclared {
    pub path: String,
    inner: ohno::OhnoCore,
}

fn main() {}
