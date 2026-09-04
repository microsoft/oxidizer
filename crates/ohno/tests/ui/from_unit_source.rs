// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Nothing converts into an error from `()`, so a `From<()>` the derive generated could only fail
//! to compile, against code the user never wrote. The source type is rejected instead.
//!
//! The entries of one `#[from(...)]` parse as a single list, so the second struct pins that the
//! rejected entry does not take the entry beside it down with it: both faults are reported.

#[derive(ohno::Error)]
#[from(())]
pub struct UnitSource {
    pub path: String,
    inner: ohno::OhnoCore,
}

#[derive(ohno::Error)]
#[from((), std::io::Error(missing: 1))]
pub struct UnitBesideAnotherFault {
    pub path: String,
    inner: ohno::OhnoCore,
}

fn main() {}
