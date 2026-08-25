// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! Implementation of the procedural macros for the [`ohno`](https://docs.rs/ohno) crate.
//!
//! This crate holds the logic behind:
//! - `#[derive(Error)]` - Automatically implement error traits
//! - `#[enrich_err("message")]` - Add error enrichment with file/line information to function errors
//! - `#[ohno::error]` - Turn a plain struct into an error type
//!
//! **Do not depend on this crate directly.** Use the re-exports from `ohno` instead.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros_impl/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros_impl/favicon.ico"
)]

mod derive_error;
mod diagnostics;
mod enrich_err;
mod error_attr;
mod marker;
mod message;
mod paths;

use proc_macro2::TokenStream;
use quote::ToTokens;

/// Expands the `#[derive(Error)]` derive macro.
#[must_use]
pub fn derive_error(input: TokenStream) -> TokenStream {
    match syn::parse2::<syn::DeriveInput>(input) {
        Ok(input) => derive_error::expand(input),
        Err(error) => error.to_compile_error(),
    }
}

/// Expands the `#[enrich_err(...)]` attribute macro.
#[must_use]
pub fn enrich_err(args: TokenStream, input: TokenStream) -> TokenStream {
    match syn::parse2::<syn::Item>(input) {
        Ok(item) => enrich_err::expand(args, item),
        Err(error) => error.to_compile_error(),
    }
}

/// Expands the `#[ohno::error]` attribute macro.
#[must_use]
pub fn error(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new_spanned(args, "`#[ohno::error]` takes no arguments")
            .to_compile_error()
            .into_token_stream();
    }

    match syn::parse2::<syn::Item>(input) {
        Ok(item) => error_attr::expand(item),
        Err(error) => error.to_compile_error(),
    }
}
