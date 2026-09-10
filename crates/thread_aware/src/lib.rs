// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), no_std)]

//! Essential building blocks for thread-per-core libraries.
//!
//! # Crate features
//!
//! * The **`std` Cargo feature** *(enabled by default)* enables the `ThreadBuilder` runtime
//!   integration API.
//! * **`derive`** *(default)* re-exports the `#[derive(ThreadAware)]` macro.
//! * Disable default features for `#![no_std]` environments. The core thread vocabulary,
//!   closures, and wrappers remain available.
//!   Enable `derive` explicitly if the derive macro is needed.
//!
//! `no_std` environments require `alloc` and pointer-width atomics.
//!
//! This crate re-exports every type from [`thread_aware_core`], which is the authoritative stable
//! relocation contract. It adds derive support, closures, wrappers, runtime thread construction,
//! and runtime thread construction.
//!
//! # Theory of Operation
//!
//! At a high level, this crate enables thread migrations of state via the [`ThreadAware`] trait:
//! - Runtimes (and similar) can use it to inform types that they were just moved across a thread or NUMA boundary.
//! - The authors of said types can then act on this information to implement performance optimizations. Such optimizations
//!   might include re-allocating memory in a new NUMA region, connecting to a thread-local I/O scheduler,
//!   or detaching from shared, possibly contended memory with the previous thread.
//!
//! Similar to [`Clone`], there are no exact semantic prescriptions of how types should behave on relocation.
//! They might continue to share some state (e.g., a common cache) or fully detach from it for performance reasons.
//! The primary goal is performance, so types should aim to minimize contention on synchronization primitives
//! and cross-NUMA memory access. Like `Clone`, the relocation itself should be mostly transparent and predictable
//! to users.
//!
//! ## Implementing [`ThreadAware`]
//!
//! In most cases [`ThreadAware`] should be implemented via the provided derive macro.
//! As thread-awareness of a type usually involves letting all contained fields know of an ongoing
//! relocation, the derive macro does just that. A default impl is provided for many `std` types,
//! so the macro should 'just work' on most compounds of built-ins.
//!
//! ## Relation to [`Send`]
//!
//! [`ThreadAware`] requires [`Send`] as a supertrait. Types are first sent to another thread,
//! then the [`ThreadAware`] relocation notification is invoked.
//!
//!
//! ## Thread vs. Core Semantics
//!
//! As this library is primarily intended for use in thread-per-core runtimes,
//! we use the terms 'thread' and 'core' interchangeably. The assumption is that items
//! primarily relocate between different threads, where each thread is pinned to a different CPU core.
//! Should a runtime utilize more than one thread per core (e.g., for internal I/O) user code should
//! be able to observe this fact.
//!
//! ## [`ThreadAware`] vs. [`Unaware`]
//!
//! Sometimes you might need to move inert types as-is, essentially bypassing all
//! thread-aware handling. These might be foreign types that carry no allocation, do
//! no I/O, or otherwise do not require any thread-specific handling.
//!
//! [`Unaware`] can be used to encapsulate such types, a wrapper that itself implements [`ThreadAware`], but
//! otherwise does not react to it. You can think of it as a `MoveAsIs<T>`. However, it was
//! deliberately named `Unaware` to signal that only types which are genuinely unaware of their
//! thread relocations (i.e., don't impl [`ThreadAware`]) should be wrapped in such.
//!
//! Wrapping types that implement the trait is discouraged, as it will prevent them from properly
//! relocating and might have an impact on their performance, but not correctness, see below.
//!
//! ## Performance vs. Correctness
//!
//! It is important to note that [`ThreadAware`] is a cooperative performance optimization and contention avoidance
//! primitive, not a guarantee of behavior for either the caller or callee. In other words, callers and runtimes must
//! continue to operate correctly if the trait is invoked incorrectly.
//!
//! In particular, [`ThreadAware`] may not always be invoked when a type leaves the current thread.
//! While runtimes should reduce the incidence of that through their API design, it may nonetheless
//! happen via [`std::thread::spawn`] and other means. In these cases types should still function
//! correctly, although they might experience degraded performance through contention of now-shared
//! resources.
//!
//! ## Provided Implementations
//!
//! [`thread_aware_core`] implements [`ThreadAware`] for core, alloc, and standard library types.
//! Implementations for third-party types live with those types once the stable trait can be adopted
//! natively. Until then, inert foreign values can be wrapped in [`Unaware`].
//!
//! # Features
//!
//! * The **`std` Cargo feature** *(enabled by default)* enables the `ThreadBuilder` runtime
//!   integration API. Disable it for `#![no_std]` environments; the crate then requires `alloc`
//!   and pointer-width atomics.
#![cfg_attr(feature = "std", doc = "  See [`ThreadBuilder`] for coordinate construction.")]
//! * **`derive`** *(default)*: Re-exports the `#[derive(ThreadAware)]` macro from the companion
//!   `thread_aware_macros` crate. Disable to avoid pulling in proc-macro code in minimal
//!   environments. For derive support without `std`, use
//!   `default-features = false, features = ["derive"]`.
//!
//! # Examples
//!
//! ## Using the [`ThreadAware` derive macro](thread_aware_macros::ThreadAware)
//!
//! When the `derive` feature (enabled by default) is active you can simply
//! use the [`ThreadAware` derive macro](thread_aware_macros::ThreadAware) instead of writing the
//! implementation manually.
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "derive")] {
//! use thread_aware::ThreadAware;
//!
//! #[derive(Debug, Clone, ThreadAware)]
//! struct Point {
//!     x: i32,
//!     y: i32,
//! }
//! # }
//! # }
//! ```
//!
//! [`thread_aware_core`]: https://docs.rs/thread_aware_core

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware/favicon.ico")]

extern crate alloc;
#[cfg(all(feature = "std", not(test)))]
extern crate std;

mod wrappers;

pub mod closure;

#[cfg(feature = "test-utils")]
#[cfg_attr(docsrs, doc(cfg(feature = "test-utils")))]
mod relocate;
#[cfg(any(test, feature = "std"))]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
mod thread;

// Re-export the derive macro (behind the `derive` feature) so users can
// simply `use thread_aware::ThreadAware;`. Disable the feature to avoid the
// proc-macro dependency in minimal builds.
/// Derive macro implementing `ThreadAware` for structs and enums.
///
/// The generated implementation transfers each field by calling its own
/// `ThreadAware::relocate` method. Fields annotated with `#[thread_aware(skip)]` are
/// left as-is (moved without invoking `relocate`).
///
/// # Supported Items
/// * Structs (named, tuple, or unit)
/// * Enums (all variant field styles)
///
/// Unions are not supported and will produce a compile error.
///
/// # Attributes
/// * `#[thread_aware(skip)]`: Prevents a field from being recursively transferred.
///
/// # Generic Bounds
/// The derive bounds each relocated field by its own type - `where <field type>: ThreadAware` -
/// not by the type parameters inside it. A field that refers back to the type being derived is the
/// exception: it bounds the parameters it reaches instead, so recursive types still compile. A
/// `#[thread_aware(skip)]` field isn't relocated, so it contributes `where Self: Send` instead,
/// which is all the `ThreadAware: Send` supertrait needs.
///
/// Bounding the field type means a `Wrapper<T>` field defers to that wrapper's own impl, so a
/// wrapper that is `ThreadAware` for every `T` keeps deriving even where `T` isn't. The derive
/// doesn't look inside function pointers, so a variance marker written as
/// `PhantomData<fn(*const T)>` adds no bound at all:
///
/// ```rust
/// # use core::marker::PhantomData;
/// # use thread_aware::ThreadAware;
/// #[derive(ThreadAware)]
/// struct Marked<T> {
///     // `PhantomData<*const T>` would make `Marked` `!Send`; this does not.
///     marker: PhantomData<fn(*const T)>,
/// }
/// ```
///
/// If you already wrote a bare `ThreadAware` bound yourself, the derive assumes it's this trait and
/// skips the duplicate; qualify the path to disambiguate.
///
/// # Example
/// ```rust
/// use thread_aware::{Thread, ThreadAware};
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
///     // This field will be moved without calling `relocate`.
///     #[thread_aware(skip)]
///     raw_len: usize,
/// }
///
/// fn demo(a1: Option<&Thread>, a2: &Thread, mut w: Wrapper) {
///     // Move the wrapper from a1 to a2.
///     w.relocate(a1, a2);
/// }
/// ```
#[cfg(any(test, feature = "derive"))]
pub use ::thread_aware_macros::ThreadAware;
#[cfg(feature = "test-utils")]
#[cfg_attr(docsrs, doc(cfg(feature = "test-utils")))]
pub use relocate::Relocator;
#[cfg(any(test, feature = "std"))]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
#[doc(inline)]
pub use thread::ThreadBuilder;
#[doc(inline)]
pub use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
pub use wrappers::{Unaware, unaware};

#[cfg(test)]
#[cfg_attr(test, mutants::skip)] // Mutating barrier coordination only deadlocks test setup.
fn test_threads(counts: &[usize]) -> Vec<Thread> {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let builder = crate::ThreadBuilder::default();
    let nodes = counts
        .iter()
        .enumerate()
        .flat_map(|(node, count)| std::iter::repeat_n(node, *count))
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(nodes.len() + 1));
    let mut handles = Vec::with_capacity(nodes.len());
    for node in nodes {
        let builder = builder
            .clone()
            .with_numa_node(node.try_into().expect("test NUMA node index must fit"));
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let coordinate = builder.build(thread::current().id());
            barrier.wait();
            coordinate
        }));
    }

    barrier.wait();
    handles
        .into_iter()
        .map(|handle| handle.join().expect("test thread should finish"))
        .collect()
}
