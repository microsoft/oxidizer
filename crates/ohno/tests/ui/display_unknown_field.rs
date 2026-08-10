// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A field that does not exist is reported by the macro, listing the fields the user declared and
//! never the `OhnoCore` injected by `#[ohno::error]`.
//!
//! The two spellings reach that report by different routes and are spanned accordingly: a named
//! placeholder is caught while parsing the template, so it is spanned at the template, while a
//! positional argument is caught while validating the argument's root, so it is spanned at the
//! offending term rather than at the whole attribute.

use std::path::PathBuf;

#[ohno::error]
#[display("bad path: {pth}")]
pub struct NamedPlaceholderError {
    pub path: PathBuf,
    pub code: i32,
}

#[ohno::error]
#[display("bad path: {}", pth.display())]
pub struct PositionalRootError {
    pub path: PathBuf,
    pub code: i32,
}

fn main() {}
