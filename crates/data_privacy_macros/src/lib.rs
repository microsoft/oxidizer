// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macros for the [`data_privacy`](https://docs.rs/data_privacy) crate.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/favicon.ico"
)]

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn taxonomy(attr_args: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::taxonomy::taxonomy(attr_args.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn classified(attr_args: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::classified::classified(attr_args.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_derive(RedactedDebug)]
#[cfg_attr(test, mutants::skip)]
pub fn redacted_debug(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::derive::redacted_debug(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_derive(RedactedDisplay)]
#[cfg_attr(test, mutants::skip)]
pub fn redacted_display(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::derive::redacted_display(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
