// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Procedural macros to support the [`data_privacy`](https://docs.rs/data_privacy) crate. See `data_privacy` for more information.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/favicon.ico"
)]

mod classified;
mod derive;
mod taxonomy;

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn taxonomy(attr_args: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    taxonomy::taxonomy_impl(attr_args.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn classified(attr_args: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    classified::classified_impl(attr_args.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_derive(RedactedDebug)]
#[cfg_attr(test, mutants::skip)]
pub fn redacted_debug(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    derive::redacted_debug_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_derive(RedactedDisplay)]
#[cfg_attr(test, mutants::skip)]
pub fn redacted_display(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    derive::redacted_display_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_derive(RedactedToString)]
#[cfg_attr(test, mutants::skip)]
pub fn redacted_to_string(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    derive::redacted_to_string_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

#[expect(missing_docs, reason = "this is documented in the data_privacy reexport")]
#[proc_macro_derive(ClassifiedDebug)]
#[cfg_attr(test, mutants::skip)]
pub fn classified_debug(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    derive::classified_debug_impl(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
