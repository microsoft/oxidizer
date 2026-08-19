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

// The modules below are public only so this crate's own integration tests can reach them.
// They are not a supported API surface: depend on `ohno` instead.
#[doc(hidden)]
pub mod derive_error;
#[doc(hidden)]
pub mod diagnostics;
#[doc(hidden)]
pub mod enrich_err;
#[doc(hidden)]
pub mod error_attr;
#[doc(hidden)]
pub mod marker;
#[doc(hidden)]
pub mod message;
#[doc(hidden)]
pub mod paths;

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
