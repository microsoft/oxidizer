// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]

//! Macros for the [`data_privacy`](https://docs.rs/data_privacy) crate.
//!
//! Macro invocation belongs in a crate that depends on the `data_privacy`
//! facade, because expansion resolves that facade crate. See `data_privacy`
//! for an example.
//!
//! ```
//! use data_privacy_macros::RedactedDebug;
//! ```

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/data_privacy_macros/favicon.ico"
)]

/// Defines a data taxonomy from an `enum`, where each variant names a data class.
///
/// The expansion gives the annotated `enum` the associated items the
/// `data_privacy` crate uses to obtain a data class for a variant. See the
/// `data_privacy` re-export of this macro for the full attribute reference.
///
/// # Errors
///
/// Malformed attribute arguments or an unsupported item shape are reported as
/// a `compile_error!` in the expansion rather than at runtime.
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn taxonomy(attr_args: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::taxonomy::taxonomy(attr_args.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Turns the annotated type into a classified container for a data class.
///
/// The expansion wraps the annotated type's value so it carries the data class
/// named by the attribute argument. See the `data_privacy` re-export of this
/// macro for the full attribute reference.
///
/// # Errors
///
/// Malformed attribute arguments or an unsupported item shape are reported as
/// a `compile_error!` in the expansion rather than at runtime.
#[proc_macro_attribute]
#[cfg_attr(test, mutants::skip)]
pub fn classified(attr_args: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::classified::classified(attr_args.into(), item.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Derives a `RedactedDebug` implementation that redacts classified fields.
///
/// Fields annotated with `#[unredacted]` keep their ordinary `Debug` output.
/// See the `data_privacy` re-export of this macro for the full attribute
/// reference.
///
/// # Errors
///
/// Malformed helper attributes or an unsupported item shape are reported as a
/// `compile_error!` in the expansion rather than at runtime.
#[proc_macro_derive(RedactedDebug, attributes(unredacted))]
#[cfg_attr(test, mutants::skip)]
pub fn redacted_debug(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::derive::redacted_debug(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Derives a `RedactedDisplay` implementation that redacts classified fields.
///
/// Fields annotated with `#[unredacted]` keep their ordinary `Display` output.
/// See the `data_privacy` re-export of this macro for the full attribute
/// reference.
///
/// # Errors
///
/// Malformed helper attributes or an unsupported item shape are reported as a
/// `compile_error!` in the expansion rather than at runtime.
#[proc_macro_derive(RedactedDisplay, attributes(unredacted))]
#[cfg_attr(test, mutants::skip)]
pub fn redacted_display(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    data_privacy_macros_impl::derive::redacted_display(input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}
