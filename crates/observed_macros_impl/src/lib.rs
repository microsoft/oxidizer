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
mod resolver;

use proc_macro2::TokenStream;
use syn::{DeriveInput, Result};

/// Expands the `#[event(...)]` attribute macro.
pub fn event(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    event_with_runtime_path(attr, item, &resolver::runtime_path())
}

/// Expands the `#[derive(Enrichment)]` derive macro.
pub fn derive_enrichment(input: TokenStream) -> Result<TokenStream> {
    derive_enrichment_with_runtime_path(input, &resolver::runtime_path())
}

/// Expands `#[event(...)]` against an explicit path to the `observed` runtime crate.
///
/// The production entry point resolves that path from the manifest of the crate
/// being compiled, which a test cannot vary. This hook takes it as an argument so
/// the expansion tests can prove that a renamed `observed` dependency reaches the
/// generated code. It is not part of the supported surface.
#[doc(hidden)]
pub fn event_with_runtime_path(attr: TokenStream, item: TokenStream, runtime: &TokenStream) -> Result<TokenStream> {
    event::event_attr(attr, item, runtime)
}

/// Expands `#[derive(Enrichment)]` against an explicit path to the `observed`
/// runtime crate. The counterpart of [`event_with_runtime_path`], and equally
/// not part of the supported surface.
#[doc(hidden)]
pub fn derive_enrichment_with_runtime_path(input: TokenStream, runtime: &TokenStream) -> Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    enrichment::derive_enrichment(&input, runtime)
}
