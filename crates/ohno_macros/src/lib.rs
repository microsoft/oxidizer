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
//! - `#[ohno::error]` - Turn a plain struct into an error type

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/ohno_macros/favicon.ico")]

use proc_macro::TokenStream;

/// Derive macro for automatically implementing error traits.
///
/// Supports the following attributes:
/// - `#[error]` - Mark the field containing the `OhnoCore`. At most one field may be marked. With
///   no marker the macro looks for a single field whose type is named `OhnoCore`, so a core reached
///   through a type alias or a renamed import has to be marked
/// - `#[display("...")]` - Custom display message with field interpolation. Positional arguments
///   are implicitly scoped to `self`, so fields are referenced by their bare name
///   (`path.display()`, not `self.path.display()`)
/// - `#[no_constructors]` - Disable automatic constructor generation. The generated `new()` and
///   `caused_by()` are `pub(crate)` even when the error type is `pub`, so an error type that needs
///   a public constructor declares one by hand. Rejected under `#[ohno::error]`, which adds the
///   `OhnoCore` field a hand-written constructor would have to initialize
/// - `#[no_debug]` - Disable automatic Debug trait implementation
/// - `#[from(Type1, Type2, ...)]` - Generate From implementations for specified types
///
/// By default, automatically implements `std::fmt::Debug` unless `#[no_debug]` is specified, so an
/// existing manual `#[derive(Debug, Error)]` collides: drop the manual `Debug` derive, or add
/// `#[no_debug]` to keep it.
///
/// See the main `ohno` crate documentation for detailed usage examples.
// The entry points are thin shims a unit test cannot invoke: a `proc_macro::TokenStream` only
// exists inside a real macro expansion. They are exercised through the `ohno` crate instead.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[proc_macro_derive(Error, attributes(error, display, no_constructors, no_debug, from))]
pub fn derive_error(input: TokenStream) -> TokenStream {
    ohno_macros_impl::derive_error(input.into()).into()
}

/// Attribute macro for adding error enrichment with file and line info to function errors.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[proc_macro_attribute]
pub fn enrich_err(args: TokenStream, input: TokenStream) -> TokenStream {
    ohno_macros_impl::enrich_err(args.into(), input.into()).into()
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[proc_macro_attribute]
pub fn error(args: TokenStream, input: TokenStream) -> TokenStream {
    ohno_macros_impl::error(args.into(), input.into()).into()
}
