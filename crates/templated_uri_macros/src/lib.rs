// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/templated_uri_macros/favicon.ico"
)]

//! Macros for the [`templated_uri`](https://docs.rs/templated_uri) crate.

use proc_macro::TokenStream;
use templated_uri_macros_impl::{raw_derive_impl, templated_paq_impl, uri_param_derive_impl};

#[proc_macro_attribute]
pub fn templated(attr: TokenStream, item: TokenStream) -> TokenStream {
    let output = templated_paq_impl(&attr.into(), item.into());
    output.into()
}

#[proc_macro_derive(Escape)]
pub fn uri_param(input: TokenStream) -> TokenStream {
    let output = uri_param_derive_impl(input.into());
    output.into()
}

#[proc_macro_derive(Raw)]
pub fn raw(input: TokenStream) -> TokenStream {
    let output = raw_derive_impl(input.into());
    output.into()
}
