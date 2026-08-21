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

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_macros_impl/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/observed_macros_impl/favicon.ico"
)]
#![expect(clippy::missing_errors_doc, reason = "This is a macro")]

mod enrichment;
mod event;
mod field_attrs;

use proc_macro2::TokenStream;
use syn::{DeriveInput, Result};

/// Expands the `#[event(...)]` attribute macro.
pub fn event(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    event::event_attr(attr, item)
}

/// Expands the `#[derive(Enrichment)]` derive macro.
pub fn derive_enrichment(input: TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    enrichment::derive_enrichment(&input)
}
