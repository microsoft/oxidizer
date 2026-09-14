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

#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_attribute]
pub fn templated(attr: TokenStream, item: TokenStream) -> TokenStream {
    let output = templated_paq_impl(&attr.into(), item.into());
    output.into()
}

#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_derive(Escape)]
pub fn uri_param(input: TokenStream) -> TokenStream {
    let output = uri_param_derive_impl(input.into());
    output.into()
}

#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_derive(Raw)]
pub fn raw(input: TokenStream) -> TokenStream {
    let output = raw_derive_impl(input.into());
    output.into()
}
