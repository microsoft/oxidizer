// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A positional argument must not carry the `self.` prefix that `thiserror` documents:
//! arguments are implicitly scoped to `self`, so this would expand to `&self.self.path`.

use std::path::PathBuf;

#[ohno::error]
#[display("bad path: {}", self.path.display())]
pub struct PrefixedError {
    pub path: PathBuf,
}

#[ohno::error]
#[display("bad path: {}", self)]
pub struct BareSelfError {
    pub path: PathBuf,
}

fn main() {}
