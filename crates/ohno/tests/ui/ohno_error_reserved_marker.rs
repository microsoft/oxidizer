// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[ohno::error]` marks the `OhnoCore` field it adds with a reserved doc comment, so a field
//! already carrying that comment was written by hand.
//!
//! One would take over the error representation from the field the attribute adds. Two would leave
//! the choice to declaration order, and neither could be named in a `#[display(...)]` template.

#[ohno::error]
#[display("failed for {path}")]
pub struct ReservedMarkerError {
    pub path: String,
    /// ohno::generated-core@7f3d9c2a
    mine: ohno::OhnoCore,
}

fn main() {}
