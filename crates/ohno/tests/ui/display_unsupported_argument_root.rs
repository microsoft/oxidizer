// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arguments are scoped to `self`, so a root that cannot follow a dot must be rejected by the
//! macro rather than reaching `rustc` as a parse error in generated code.

#[ohno::error]
#[display("bad: {}", Self::LABEL.len())]
pub struct QualifiedPathRootError {
    pub path: String,
}

#[ohno::error]
#[display("bad: {}", "prefix".len())]
pub struct StringLiteralRootError {
    pub path: String,
}

fn main() {}
