// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[error]` marks the field holding the `OhnoCore` and takes no arguments, so anything that is
//! not the bare marker is reported rather than ignored. That includes `#[error(generated)]`, the
//! form the macro once wrote for itself and a user could copy out of an expansion.

#[derive(ohno::Error)]
pub struct UnknownArgumentError {
    pub path: String,
    #[error(nonsense)]
    inner: ohno::OhnoCore,
}

#[derive(ohno::Error)]
pub struct NameValueError {
    pub path: String,
    #[error = "nonsense"]
    inner: ohno::OhnoCore,
}

#[derive(ohno::Error)]
pub struct CopiedFromExpansionError {
    pub path: String,
    #[error(generated)]
    inner: ohno::OhnoCore,
}

fn main() {}
