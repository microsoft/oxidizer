// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros/favicon.ico")]

//! Macros for the [`templated_uri`](https://docs.rs/templated_uri) crate.

use proc_macro::TokenStream;
use templated_uri_macros_impl::{templated_paq_impl, uri_fragment_derive_impl, uri_unsafe_fragment_derive_impl};

#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_attribute]
pub fn templated(attr: TokenStream, item: TokenStream) -> TokenStream {
    let output = templated_paq_impl(&attr.into(), item.into());
    output.into()
}

#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_derive(UriFragment)]
pub fn uri_fragment(input: TokenStream) -> TokenStream {
    let output = uri_fragment_derive_impl(input.into());
    output.into()
}

#[cfg_attr(test, mutants::skip)] // The macro is tested indirectly through the `templated_uri` crate's tests, so we can skip it in mutation testing here.
#[proc_macro_derive(UriUnsafeFragment)]
pub fn uri_unsafe_fragment(input: TokenStream) -> TokenStream {
    let output = uri_unsafe_fragment_derive_impl(input.into());
    output.into()
}
