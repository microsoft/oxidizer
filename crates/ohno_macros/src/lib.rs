// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Procedural macros to support the [`ohno`](https://docs.rs/ohno) crate. See `ohno` for more information.
//!
//! # Macros
//!
//! - `#[derive(Error)]` - Automatically implement error traits
//! - `#[error_span("message")]` - Add error trace with file/line information to function errors

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/favicon.ico")]

use proc_macro::TokenStream;

mod derive_error;
mod error_span;
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
///
/// By default, automatically implements `std::fmt::Debug` unless `#[no_debug]` is specified.
/// This means existing code with manual `#[derive(Debug, Error)]` will have conflicts and
/// should either remove the manual Debug derive or add `#[no_debug]` to preserve the manual implementation.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_derive(Error, attributes(error, display, no_constructors, no_debug, from))]
#[cfg_attr(test, mutants::skip)]
pub fn derive_error(input: TokenStream) -> TokenStream {
    derive_error::derive_error(input)
}

/// Attribute macro for adding error trace with file and line info to function errors.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn error_span(args: TokenStream, input: TokenStream) -> TokenStream {
    error_span::error_span(args, input)
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
