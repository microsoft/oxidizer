// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Macros for the [`thread_aware`](https://docs.rs/thread_aware) crate.
//!
//! # Provided Derives
//!
//! * `#[derive(ThreadAware)]` – Auto-implements the `thread_aware::ThreadAware` trait by recursively
//!   calling `transfer` on each field.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_macros/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_macros/favicon.ico"
)]

use proc_macro::TokenStream;
use syn::{Path, parse_quote};

#[proc_macro_derive(ThreadAware, attributes(thread_aware))]
#[cfg_attr(test, mutants::skip)]
#[expect(missing_docs, reason = "Documented in the thread_aware crate's reexport")]
pub fn derive_transfer(input: TokenStream) -> TokenStream {
    let root_path: Path = parse_quote!(::thread_aware);
    thread_aware_macros_impl::derive_thread_aware(input.into(), &root_path).into()
}
