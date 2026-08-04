// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A `{placeholder}` naming a field that does not exist must be reported against the template,
//! listing the fields the user declared and never the `OhnoCore` injected by `#[ohno::error]`.

use std::path::PathBuf;

#[ohno::error]
#[display("bad path: {pth}")]
pub struct UnknownPlaceholderError {
    pub path: PathBuf,
    pub code: i32,
}

fn main() {}
