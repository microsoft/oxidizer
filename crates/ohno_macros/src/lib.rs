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
//! The implementation is being rewritten. This file declares the public surface only; every entry
//! point is unimplemented. See `docs/requirements.md` for what the rewrite has to deliver and
//! `docs/design.md` for the shape it is being rewritten into.

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
/// - `#[no_constructors]` - Disable automatic constructor generation
/// - `#[no_debug]` - Disable automatic Debug trait implementation
/// - `#[from(Type1, Type2, ...)]` - Generate From implementations for specified types
///
/// By default, automatically implements `std::fmt::Debug` unless `#[no_debug]` is specified.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_derive(Error, attributes(error, display, no_constructors, no_debug, from))]
pub fn derive_error(_input: TokenStream) -> TokenStream {
    unimplemented!("ohno_macros is being rewritten; see crates/ohno_macros/docs/requirements.md")
}

/// Attribute macro for adding error enrichment with file and line info to function errors.
///
/// See the main `ohno` crate documentation for detailed usage examples.
#[proc_macro_attribute]
pub fn enrich_err(_args: TokenStream, _input: TokenStream) -> TokenStream {
    unimplemented!("ohno_macros is being rewritten; see crates/ohno_macros/docs/requirements.md")
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
pub fn error(_args: TokenStream, _input: TokenStream) -> TokenStream {
    unimplemented!("ohno_macros is being rewritten; see crates/ohno_macros/docs/requirements.md")
}
