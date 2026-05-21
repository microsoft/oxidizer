// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Using `Option<String>` in a restricted template position (`{param}`) must fail
//! to compile because `String` does not implement `Escape`.
//!
//! The extra blank lines below shift both diagnostic line numbers (the `#[templated]`
//! attribute and the `Option<String>` field) to 2-digit values. rustc pads line-number
//! columns in a diagnostic block to the width of the widest reference; keeping both
//! references at the same digit count produces stable column alignment in the
//! `.stderr` regardless of how rustc decides to pad.

use templated_uri::templated;

#[templated(template = "/users/{user_id}", unredacted)]
#[derive(Clone)]
struct UserPath {
    user_id: Option<String>,
}

fn main() {}
