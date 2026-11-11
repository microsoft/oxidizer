// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Attribute macros for [`arty`](https://docs.rs/arty) runtime entry points.
//!
//! Enable the `macros` feature of Arty. Then use `#[arty::main]` for an
//! application or `#[arty::test]` for a test. You do not need to add a direct
//! dependency on this companion crate.
//!
//! Both attributes start an Arty runtime. They pass an
//! [`arty::task::Context`](https://docs.rs/arty/latest/arty/task/struct.Context.html)
//! to the async function. The runtime stops when the function returns.
//!
//! `workers = N` starts exactly `N` async workers. If `N` exceeds the available
//! processor capacity, multiple workers share processors. `workers = all`
//! starts one worker for each available processor. Both attributes use one
//! worker by default.
//!
//! `monitor = enabled` or `monitor = disabled` selects whether the runtime is
//! prepared for monitoring. `#[arty::main]` enables it by default.
//! `#[arty::test]` disables it by default. `panic = shutdown` or
//! `panic = ignore` selects whether the runtime stops after Arty catches a task
//! panic. Both attributes use `shutdown` by default.
//!
//! See [`arty::main`](https://docs.rs/arty/latest/arty/attr.main.html) and
//! [`arty::test`](https://docs.rs/arty/latest/arty/attr.test.html) for signatures,
//! configuration, examples, and failure conditions.

use proc_macro::TokenStream;

/// Runs an async entry point on an Arty runtime.
///
/// See [`arty::main`](https://docs.rs/arty/latest/arty/attr.main.html) for its
/// signature, configuration, examples, and panic behavior.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Thin proc-macro shim. arty_macros_impl tests the expansion logic.
pub fn main(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::main(args.into(), item.into()).into()
}

/// Runs an async test on an Arty runtime.
///
/// See [`arty::test`](https://docs.rs/arty/latest/arty/attr.test.html) for its
/// signature, configuration, controlled-time examples, and panic behavior.
#[proc_macro_attribute]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Thin proc-macro shim. arty_macros_impl tests the expansion logic.
pub fn test(args: TokenStream, item: TokenStream) -> TokenStream {
    arty_macros_impl::test(args.into(), item.into()).into()
}
