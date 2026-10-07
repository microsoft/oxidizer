// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Entry-point macros for [`arty`](https://docs.rs/arty).
//!
//! Enable Arty's `macros` feature and use `#[arty::main]` or `#[arty::test]`.
//! You do not need to depend on this companion crate directly.
//!
//! Both attributes start a runtime, pass its worker capabilities to an
//! async function, and shut down when that function returns. The optional
//! `workers = N` argument caps the runtime's async worker count.
//!
//! The application-facing references are
//! [`arty::main`](https://docs.rs/arty/latest/arty/attr.main.html) and
//! [`arty::test`](https://docs.rs/arty/latest/arty/attr.test.html). They contain
//! runnable examples, configuration syntax, and failure conditions.

use proc_macro::TokenStream;

/// Runs an async entry point on an Arty runtime.
///
/// See [`arty::main`](https://docs.rs/arty/latest/arty/attr.main.html) for the
/// required signature, configuration, examples, and panic behavior.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::main(args.into(), item.into()).into()
}

/// Runs an async test on an Arty runtime.
///
/// See [`arty::test`](https://docs.rs/arty/latest/arty/attr.test.html) for the
/// required signature, configuration, controlled-time examples, and panic behavior.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::test(args.into(), item.into()).into()
}
