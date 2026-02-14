// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/CRATE_NAME/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/CRATE_NAME/favicon.ico")]

//! Macros for the [`obscuri`](https://docs.rs/obscuri) crate.

use obscuri_macros_impl::{templated_paq_impl, uri_fragment_derive_impl, uri_unsafe_fragment_derive_impl};
use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn templated(attr: TokenStream, item: TokenStream) -> TokenStream {
    let output = templated_paq_impl(&attr.into(), item.into());
    output.into()
}

#[proc_macro_derive(UriFragment)]
pub fn uri_fragment(input: TokenStream) -> TokenStream {
    let output = uri_fragment_derive_impl(input.into());
    output.into()
}

#[proc_macro_derive(UriUnsafeFragment)]
pub fn uri_unsafe_fragment(input: TokenStream) -> TokenStream {
    let output = uri_unsafe_fragment_derive_impl(input.into());
    output.into()
}
