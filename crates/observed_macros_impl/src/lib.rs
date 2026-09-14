// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Implementation of the procedural macros for the `observed` crate.
//!
//! This crate holds the logic behind:
//! - `#[event(...)]` - generate an `Event` trait impl for a struct
//! - `#[derive(Enrichment)]` - generate an `Enrichment` trait impl for a struct
//!
//! **Do not depend on this crate directly.** Use the re-exports from `observed` instead.
//!
//! ```
//! use observed_macros_impl::{derive_enrichment, event};
//! ```

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_macros_impl/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_macros_impl/favicon.ico"
)]

mod enrichment;
mod event;
mod field_attrs;

use proc_macro2::TokenStream;
use syn::{DeriveInput, Result};

/// Expands the `#[event(...)]` attribute macro.
///
/// # Errors
///
/// Returns a [`syn::Error`] when the attribute arguments are malformed or when
/// the annotated item is not a struct with named fields.
pub fn event(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    event::event_attr(attr, item)
}

/// Expands the `#[derive(Enrichment)]` derive macro.
///
/// # Errors
///
/// Returns a [`syn::Error`] when the input does not parse as a derive item,
/// when it is not a struct with named fields, or when a field carries a
/// helper attribute the macro rejects.
pub fn derive_enrichment(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    enrichment::derive_enrichment(&input)
}
