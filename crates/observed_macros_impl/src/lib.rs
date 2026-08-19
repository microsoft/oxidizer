// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
#![expect(
    clippy::must_use_candidate,
    clippy::too_long_first_doc_paragraph,
    reason = "Internal items, public only so this crate's integration tests can reach them"
)]

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

/// Internal items reached only by this crate's own integration tests. Not a public API.
#[doc(hidden)]
pub mod internals {
    pub use crate::enrichment::derive_enrichment;
    pub use crate::event::{
        EventArgs, NumericKind, SeverityKind, event_attr, generate_event, is_128_bit_int, numeric_kind, strip_helper_attrs,
        strip_type_wrappers,
    };
    pub use crate::field_attrs::{mentions_any_type_param, option_inner_type, strip_reference};
}
