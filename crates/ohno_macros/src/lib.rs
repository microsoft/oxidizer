// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`ohno`](https://docs.rs/ohno) crate.
//!
//! # Macros
//!
//! - `#[derive(Error)]` - Automatically implement error traits
//! - `#[enrich_err("message")]` - Add error enrichment with file/line information to function errors

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/favicon.ico")]

use proc_macro::TokenStream;

mod derive_error;
mod enrich_err;
mod error_type_attr;
mod utils;

/// Derive macro for automatically implementing error traits.
///
/// Supports the following attributes:
/// - `#[error]` - Mark the field containing the `OhnoCore`
/// - `#[display("...")]` - Custom display message with field interpolation
/// - `#[no_constructors]` - Disable automatic constructor generation
/// - `#[no_debug]` - Disable automatic Debug trait implementation
/// - `#[from(Type1, Type2, ...)]` - Generate From implementations for specified types
/// - `#[backtrace(force)]` - Force backtrace capture regardless of environment
/// - `#[backtrace(disabled)]` - Disable backtrace capture
///
/// By default, automatically implements `std::fmt::Debug` unless `#[no_debug]` is specified.
/// This means existing code with manual `#[derive(Debug, Error)]` will have conflicts and
/// should either remove the manual Debug derive or add `#[no_debug]` to preserve the manual implementation.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_derive(Error, attributes(error, display, no_constructors, no_debug, from, backtrace))]
#[cfg_attr(test, mutants::skip)]
pub fn derive_error(input: TokenStream) -> TokenStream {
    derive_error::derive_error(input)
}

/// Attribute macro for adding error enrichment with file and line info to function errors.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn enrich_err(args: TokenStream, input: TokenStream) -> TokenStream {
    enrich_err::enrich_err(args, input)
}

/// Attribute macro version of `error_type` that preserves documentation comments.
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
#[cfg_attr(test, mutants::skip)]
pub fn error(args: TokenStream, input: TokenStream) -> TokenStream {
    error_type_attr::error(args, input)
}
