// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A positional argument rooted in a field that does not exist must be reported against that
//! argument, rather than reaching `rustc` as a field access on the expanded struct.

use std::path::PathBuf;

#[ohno::error]
#[display("bad path: {}", pth.display())]
pub struct UnknownArgumentRootError {
    pub path: PathBuf,
    pub code: i32,
}

fn main() {}
