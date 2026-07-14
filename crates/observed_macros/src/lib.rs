// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Procedural macros for the `observed` crate.
//!
//! This crate provides:
//! - `#[derive(Event)]` - generate an `Event` trait impl for a struct
//! - `#[derive(Enrichment)]` - generate an `Enrichment` trait impl for a struct
//!
//! **Do not depend on this crate directly.** Use the re-exports from `observed` instead.

mod enrichment;
mod event;
mod field_attrs;

use proc_macro::TokenStream;

/// Derives the `Event` trait for a struct. See the re-export in the `observed`
/// crate for full documentation.
#[proc_macro_derive(Event, attributes(event, log, metric, disabled, dimension, unredacted, data_class, if_none))]
pub fn derive_event(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match crate::event::derive_event(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derives the `Enrichment` trait for a struct. See the re-export in the `observed`
/// crate for full documentation.
#[proc_macro_derive(Enrichment, attributes(dimension, unredacted, data_class, if_none))]
pub fn derive_enrichment(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match crate::enrichment::derive_enrichment(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
