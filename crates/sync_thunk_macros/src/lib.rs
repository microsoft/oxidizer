// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`sync_thunk`](https://docs.rs/sync_thunk) crate.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/sync_thunk_macros/favicon.ico"
)]

#[expect(missing_docs, reason = "this is documented in the sync_thunk reexport")]
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn thunk(attr_args: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    sync_thunk_macros_impl::thunk_impl(attr_args.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
