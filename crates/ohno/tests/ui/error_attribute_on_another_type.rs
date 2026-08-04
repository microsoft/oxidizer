// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `#[error]` marks the field holding the `OhnoCore`, so a field of another type carrying it is
//! reported against the field itself.
//!
//! Without this the mistake surfaced only through the implementations generated from that field,
//! as a series of errors about methods the user never called, spanned at the derive rather than
//! at anything they wrote.

#[derive(ohno::Error)]
pub struct GeneratedMarkerOnAnotherType {
    #[error(generated)]
    hidden: String,
    inner: ohno::OhnoCore,
}

#[derive(ohno::Error)]
pub struct BareMarkerOnAnotherType {
    #[error]
    not_a_core: String,
}

fn main() {}
