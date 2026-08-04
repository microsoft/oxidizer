// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]` adds the `OhnoCore` field itself, so a struct that marks one of its own would
//! end up holding two, with the choice between them left to declaration order.

#[ohno::error]
pub struct AlreadyMarked {
    pub path: String,
    #[error]
    inner: ohno::OhnoCore,
}

fn main() {}
