// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Only one field holds the `OhnoCore`, so marking a second leaves the choice to declaration
//! order. The macro reports it rather than resolving it silently.

#[derive(ohno::Error)]
pub struct TwoMarkedFields {
    #[error]
    first: ohno::OhnoCore,
    #[error]
    second: ohno::OhnoCore,
}

#[derive(ohno::Error)]
pub struct TwoMarkedTupleFields(#[error] ohno::OhnoCore, #[error] ohno::OhnoCore);

fn main() {}
