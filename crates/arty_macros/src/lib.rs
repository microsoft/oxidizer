// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Entry-point macros for [`arty`](https://docs.rs/arty).
//!
//! Enable the `macros` feature in Arty and use `#[arty::rt::main]` or `#[arty::rt::test]`.
//! Each annotated asynchronous function takes one `arty::rt::Builtins` argument.
//! Use `runtime_path = ::renamed_arty::rt` for a renamed or re-exported runtime.

use proc_macro::TokenStream;

/// Runs an asynchronous entry point on an Arty runtime.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::main(args.into(), item.into()).into()
}

/// Runs an asynchronous test on an Arty runtime, preserving test attributes.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::test(args.into(), item.into()).into()
}
