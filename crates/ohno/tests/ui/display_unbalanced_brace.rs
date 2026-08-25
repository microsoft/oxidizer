// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An unbalanced brace in a display template is reported by the macro rather than parsed into a
//! different, valid template.
//!
//! A `{` with no matching `}` would otherwise run to the end of the template and be honored as a
//! placeholder, so `"{path"` would render like `"{path}"` and the typo would never surface. A `}`
//! with no matching `{` would otherwise be copied into the generated `format!` string, where rustc
//! reports it in code the user cannot see.

use std::path::PathBuf;

#[ohno::error]
#[display("bad path: {path")]
pub struct UnterminatedPlaceholderError {
    pub path: String,
}

#[ohno::error]
#[display("bad path: path}")]
pub struct UnmatchedClosingBraceError {
    pub path: PathBuf,
}

fn main() {}
