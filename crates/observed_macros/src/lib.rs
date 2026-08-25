// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_macros/favicon.ico"
)]

//! Procedural macros for the `observed` crate.
//!
//! This crate provides:
//! - `#[event(...)]` - generate an `Event` trait impl for a struct
//! - `#[derive(Enrichment)]` - generate an `Enrichment` trait impl for a struct
//!
//! **Do not depend on this crate directly.** Use the re-exports from `observed` instead.

mod enrichment;
mod event;
mod field_attrs;

use proc_macro::TokenStream;

/// Declares a struct as an `observed` event and generates its `Event` trait impl.
/// See the re-export in the `observed` crate for full documentation.
///
/// This is an attribute macro (not a derive): it consumes the sibling
/// log-severity (`#[info]`, `#[warning]`, ...) and metric-kind (`#[gauge]`, ...)
/// attributes, strips them from the re-emitted struct, and generates the impl.
/// The `warn` severity is spelled `#[warning(...)]` because `warn` is a built-in
/// lint attribute that cannot be used as a custom attribute.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // a proc-macro entry point cannot be invoked from this crate's own tests
pub fn event(attr: TokenStream, item: TokenStream) -> TokenStream {
    match crate::event::event_attr(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derives the `Enrichment` trait for a struct. See the re-export in the `observed`
/// crate for full documentation.
#[proc_macro_derive(Enrichment, attributes(dimension, unredacted, data_class, if_none))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // a proc-macro entry point cannot be invoked from this crate's own tests
pub fn derive_enrichment(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match crate::enrichment::derive_enrichment(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
