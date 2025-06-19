// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Marks async function to be executed by the oxidizer runtime. Note that a more fully featured version
/// of this macro is available in the `oxidizer_app` crate.
///
/// # Usage
///
/// ```
/// use oxidizer_rt::{main, BasicThreadState};
/// #[main]
/// async fn main(cx: BasicThreadState) {
///     println!("Hello, world!");
///
///     cx.builtins().task_scheduler.spawn(async move |_| {
///         println!("Hello again!");
///     }).await;
/// }
/// ```
#[cfg(feature = "macros")]
pub use oxidizer_macros::__macro_runtime_main as main;
/// Marks async test function to be executed by the oxidizer runtime.
/// Usage is similar to [`main`] macro (see above or in `tests/test_macro.rs` integration test)
#[cfg(feature = "macros")]
pub use oxidizer_macros::__macro_runtime_test as test;