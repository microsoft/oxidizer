// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`ohno`](https://docs.rs/ohno) crate.
//!
//! # Macros
//!
//! - `#[derive(Error)]` - Automatically implement error traits
//! - `#[enrich_err("message")]` - Add error enrichment with file/line information to function errors
//! - `#[ohno::error]` - Turn a plain struct into an error type
//!
//! # Status
//!
//! See `docs/requirements.md` for what the crate has to deliver and `docs/design.md` for the shape
//! it is built in.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/favicon.ico")]

mod derive_error;
mod diagnostics;
mod enrich_err;
mod error_attr;
mod marker;
mod message;
mod paths;

use proc_macro::TokenStream;
use quote::ToTokens;

/// Derive macro for automatically implementing error traits.
///
/// Supports the following attributes:
/// - `#[error]` - Mark the field containing the `OhnoCore`. At most one field may be marked. With
///   no marker the macro looks for a single field whose type is named `OhnoCore`, so a core reached
///   through a type alias or a renamed import has to be marked
/// - `#[display("...")]` - Custom display message with field interpolation. Positional arguments
///   are implicitly scoped to `self`, so fields are referenced by their bare name
///   (`path.display()`, not `self.path.display()`)
/// - `#[no_constructors]` - Disable automatic constructor generation
/// - `#[no_debug]` - Disable automatic Debug trait implementation
/// - `#[from(Type1, Type2, ...)]` - Generate From implementations for specified types
///
/// By default, automatically implements `std::fmt::Debug` unless `#[no_debug]` is specified.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_derive(Error, attributes(error, display, no_constructors, no_debug, from))]
pub fn derive_error(input: TokenStream) -> TokenStream {
    match syn::parse::<syn::DeriveInput>(input) {
        Ok(input) => derive_error::expand(input).into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Attribute macro for adding error enrichment with file and line info to function errors.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_attribute]
pub fn enrich_err(args: TokenStream, input: TokenStream) -> TokenStream {
    match syn::parse::<syn::Item>(input) {
        Ok(item) => enrich_err::expand(args.into(), item).into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Attribute macro that adds the `OhnoCore` field to a struct and derives the error traits.
///
/// This allows using regular Rust doc comments with error types:
///
/// ```ignore
/// /// Documentation for MyError
/// #[ohno::error]
/// struct MyError;
/// ```
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_attribute]
pub fn error(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        let args: proc_macro2::TokenStream = args.into();
        return syn::Error::new_spanned(args, "`#[ohno::error]` takes no arguments")
            .to_compile_error()
            .into_token_stream()
            .into();
    }

    match syn::parse::<syn::Item>(input) {
        Ok(item) => error_attr::expand(item).into(),
        Err(error) => error.to_compile_error().into(),
    }
}
