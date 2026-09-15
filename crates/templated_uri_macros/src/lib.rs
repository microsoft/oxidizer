// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Procedural macros that implement the URI template attributes and derives of
//! the [`templated_uri`](https://docs.rs/templated_uri) crate.
//!
//! Macro invocation belongs in a crate that depends on the `templated_uri`
//! facade, because expansion resolves that facade crate. See `templated_uri`
//! for an example.
//!
//! ```
//! use templated_uri_macros::Raw;
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros/favicon.ico"
)]

use proc_macro::TokenStream;
use templated_uri_macros_impl::{raw_derive_impl, templated_paq_impl, uri_param_derive_impl};

/// Implements the URI template traits for a struct or enum.
///
/// The attribute is re-exported as `templated_uri::templated`, whose
/// documentation describes the template syntax and the supported attributes.
///
/// # Errors
///
/// The macro emits a `compile_error!` instead of returning an error: generic
/// types, unions, malformed attribute arguments, and templates the parser
/// rejects all fail when the expansion is compiled.
#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_attribute]
pub fn templated(attr: TokenStream, item: TokenStream) -> TokenStream {
    let output = templated_paq_impl(&attr.into(), item.into());
    output.into()
}

/// Derives the `Escape` trait for a newtype wrapping a URI-escaped value.
///
/// The derive is re-exported as `templated_uri::Escape`, whose documentation
/// describes the requirements it places on the annotated type.
///
/// # Errors
///
/// The macro emits a `compile_error!` instead of returning an error: input
/// that does not parse, generic types, enums, unions, and tuple structs
/// without exactly one field all fail when the expansion is compiled.
#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_derive(Escape)]
pub fn uri_param(input: TokenStream) -> TokenStream {
    let output = uri_param_derive_impl(input.into());
    output.into()
}

/// Derives the `Raw` trait for a newtype wrapping an unescaped value.
///
/// The derive is re-exported as `templated_uri::Raw`, whose documentation
/// describes the requirements it places on the annotated type.
///
/// # Errors
///
/// The macro emits a `compile_error!` instead of returning an error: input
/// that does not parse, generic types, enums, unions, and tuple structs
/// without exactly one field all fail when the expansion is compiled.
#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_derive(Raw)]
pub fn raw(input: TokenStream) -> TokenStream {
    let output = raw_derive_impl(input.into());
    output.into()
}
