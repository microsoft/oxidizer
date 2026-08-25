// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field names reach the diagnostics as text, so a raw identifier is echoed back with its prefix:
//! `r#type` is what the user must write, and offering `type` would name something that does not
//! parse where they need it.

#[ohno::error]
#[display("bad: {typ}")]
pub struct RawIdentifierFieldError {
    pub r#type: String,
}

fn main() {}
