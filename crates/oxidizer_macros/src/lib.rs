// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! <div class="warning">This crate is a private dependency of <b>oxidizer</b> crate.</div>

#![doc(hidden)]
#![doc(html_no_source)]

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn __macro_stability(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::stability::entrypoint(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_api(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::api::entrypoint(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_traverse(_attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::traverse::entrypoint(item.into()).into()
}

#[proc_macro_derive(__macro_derive_context)]
pub fn __macro_derive_context(input: TokenStream) -> TokenStream {
    oxidizer_macros_impl::context::entrypoint(input.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_runtime_main(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::runtime::impl_runtime_main(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_app_main(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::runtime::impl_app_main(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_oxidizer_app_main(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::runtime::impl_oxidizer_app_main(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_runtime_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::runtime::impl_runtime_test(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_app_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::runtime::impl_app_test(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn __macro_oxidizer_app_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    oxidizer_macros_impl::runtime::impl_oxidizer_app_test(attr.into(), item.into()).into()
}