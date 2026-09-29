// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Runs an asynchronous entry point on an Arty runtime.
///
/// The function takes one [`Builtins`](crate::rt::Builtins) argument. Its return value
/// becomes the synchronous entry point's return value.
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::rt::main]
/// async fn main(cx: arty::rt::Builtins) {
///     cx.scheduler().spawn(async |_| {}).await;
/// }
/// # #[cfg(not(feature = "macros"))] fn main() {}
/// ```
///
/// Use `runtime_path = ::renamed_arty::rt` when the crate is renamed or re-exported.
pub use arty_macros::main;
/// Runs an asynchronous test on an Arty runtime.
///
/// The function takes one [`Builtins`](crate::rt::Builtins) argument. Standard test
/// attributes, such as `#[should_panic]` and `#[ignore]`, are preserved.
///
/// ```
/// # #[cfg(feature = "macros")]
/// #[arty::rt::test]
/// async fn answer(cx: arty::rt::Builtins) {
///     assert_eq!(cx.scheduler().spawn(async |_| 42).await, 42);
/// }
/// ```
///
/// Use `runtime_path = ::renamed_arty::rt` when the crate is renamed or re-exported.
pub use arty_macros::test;
