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

/// Derive macro implementing `ThreadAware` for structs and enums.
///
/// The generated implementation transfers each field by calling its own
/// `ThreadAware::relocated` method. Fields annotated with `#[transfer(skip)]` are
/// left as-is (moved without invoking `transfer`).
///
/// # Supported Items
/// * Structs (named, tuple, or unit)
/// * Enums (all variant field styles)
///
/// Unions are not supported and will produce a compile error.
///
/// # Attributes
/// * `#[thread_aware(skip)]` – Prevents a field from being recursively transferred.
///
/// # Generic Bounds
/// Generic type parameters appearing in non-skipped fields automatically receive a
/// `::thread_aware::ThreadAware` bound (occurrences only inside `PhantomData<..>` are ignored).
///
/// # Example
/// ```rust
/// use thread_aware::{MemoryAffinity, ThreadAware};
///
/// #[derive(ThreadAware)]
/// struct Payload {
///     id: u64,
///     data: Vec<u8>,
/// }
///
/// #[derive(ThreadAware)]
/// struct Wrapper {
///     // This field will be recursively transferred.
///     inner: Payload,
///     // This field will be moved without calling `transfer`.
///     #[thread_aware(skip)]
///     raw_len: usize,
/// }
///
/// fn demo(mut a1: MemoryAffinity, mut a2: MemoryAffinity, w: Wrapper) -> Wrapper {
///     // Move the wrapper from a1 to a2.
///     let moved = w.relocated(a1.clone(), a2.clone());
///     moved
/// }
/// ```
#[proc_macro_derive(ThreadAware, attributes(thread_aware))]
#[cfg_attr(test, mutants::skip)]
pub fn derive_transfer(input: TokenStream) -> TokenStream {
    let root_path: Path = parse_quote!(::thread_aware);
    thread_aware_macros_impl::derive_thread_aware(input.into(), &root_path).into()
}
