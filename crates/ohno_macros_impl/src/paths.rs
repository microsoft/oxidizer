// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The paths generated code refers to.
//!
//! Generated code names the crate `ohno`. Renaming the package in `Cargo.toml` is not supported.
//! The leading `::` is safe inside `ohno` itself, which declares `extern crate self as ohno`.

use proc_macro2::TokenStream;
use quote::quote;

/// `::ohno::OhnoCore`, the type held by the error field.
#[must_use]
pub(crate) fn ohno_core() -> TokenStream {
    quote!(::ohno::OhnoCore)
}

/// `::ohno::Enrichable`, the trait carrying `add_enrichment`.
#[must_use]
pub(crate) fn enrichable() -> TokenStream {
    quote!(::ohno::Enrichable)
}

/// `::ohno::EnrichmentEntry`, one message with its source location.
#[must_use]
pub(crate) fn enrichment_entry() -> TokenStream {
    quote!(::ohno::EnrichmentEntry)
}

/// `::ohno::ErrorExt`, the trait carrying `message` and `backtrace`.
#[must_use]
pub(crate) fn error_ext() -> TokenStream {
    quote!(::ohno::ErrorExt)
}
