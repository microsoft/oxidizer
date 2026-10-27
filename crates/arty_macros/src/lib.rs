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
//! `workers = N` argument starts exactly `N` async worker threads. Multiple
//! workers share processors when `N` exceeds available processor capacity;
//! `workers = all` selects all available processors. Both attributes use one
//! worker by default.
//!
//! The `monitor = enabled` and `monitor = disabled` arguments select the
//! monitoring policy. `#[arty::main]` defaults to enabled. `#[arty::test]`
//! defaults to disabled so future monitor activation stays out of ordinary tests.
//! The `panic = shutdown` and `panic = ignore` arguments select the runtime
//! panic policy. Both attributes default to shutdown.
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
#[cfg_attr(test, mutants::skip)] // Thin proc-macro shim; expansion logic is tested in arty_macros_impl.
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::main(args.into(), item.into()).into()
}

/// Runs an async test on an Arty runtime.
///
/// See [`arty::test`](https://docs.rs/arty/latest/arty/attr.test.html) for the
/// required signature, configuration, controlled-time examples, and panic behavior.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Thin proc-macro shim; expansion logic is tested in arty_macros_impl.
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::test(args.into(), item.into()).into()
}
