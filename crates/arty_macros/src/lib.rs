// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Entry-point macros for [`arty`](https://docs.rs/arty).
//!
//! Enable the `macros` feature in Arty and use `#[arty::runtime::main]` or `#[arty::runtime::test]`.
//! Each annotated asynchronous function takes one owned `arty::runtime::Builtins` argument.
//! Use `runtime_path = ::renamed_arty::runtime` for a renamed or re-exported runtime.
//!
//! - `workers = N` limits asynchronous workers to at most the nonzero integer literal `N`.
//!   This does not limit blocking-task pools or change the runtime's default when omitted.
//! - `builder = expression` uses an existing `arty::runtime::RuntimeBuilder`, evaluated once
//!   on the calling thread. It cannot be combined with `workers`.
//! - Tests may opt into simulated time by adding an owned `arty::time::ClockControl` as
//!   their second parameter. This requires Arty's `test-util` feature, starts with manual
//!   advancement, and cannot be combined with `builder`.
//!
//! See the [`main`](https://docs.rs/arty/latest/arty/runtime/attr.main.html) and
//! [`test`](https://docs.rs/arty/latest/arty/runtime/attr.test.html) documentation in Arty
//! for examples, clock semantics, and construction-error behavior.

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
