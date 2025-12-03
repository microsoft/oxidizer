// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! A library for creating isolated affinities in Rust, allowing for safe and efficient data transfer between them.
//!
//! This is useful in scenarios where you want to isolate data to different affinities, such as NUMA nodes, threads, or specific CPU cores.
//! It can serve as a foundation for building a async runtime or a task scheduler that operates across multiple affinities.
//! For example it can be used to implement a NUMA-aware task scheduler that transfers data between different NUMA nodes
//! or a thread-per-core scheduler that transfers data between different CPU cores.
//!
//! The way this would be used is by restricting how work can be scheduled on other affinities (threads,
//! NUMA nodes). If the runtime only allows work scheduling in a way that accepts work that can be
//! transferred (e.g. by using the [`RelocateFnOnce`] trait) and makes sure that transfer is called, it can
//! effectively isolate the affinities as the [`ThreadAware`] trait ensures the right level of separation if
//! implemented correctly.
//!
//! `ThreadAware` is an 'infectious' trait, meaning that when you implement it for a type,
//! all of its fields must also implement [`ThreadAware`] and you must call their `transfer` methods.
//! However [`ThreadAware`] is provided for many common types, so you can use it out of the box for most cases.
//!
//! # Feature Flags
//! * **`derive`** *(default)* – Re-exports the `#[derive(ThreadAware)]` macro from the companion
//!   `thread_aware_macros` crate. Disable to avoid pulling in proc-macro code in minimal
//!   environments: `default-features = false`.
//!
//! # Examples
//!
//! ```rust
//! use thread_aware::{PinnedAffinity, MemoryAffinity, ThreadAware, Unaware, create_manual_pinned_affinities};
//!
//! // Define a type that implements ThreadAware
//! #[derive(Debug, Clone)]
//! struct MyData {
//!     value: i32,
//! }
//!
//! impl ThreadAware for MyData {
//!     fn relocated(mut self, source: MemoryAffinity, destination: PinnedAffinity) -> Self {
//!         self.value = self.value.relocated(source, destination);
//!         self
//!     }
//! }
//!
//! fn do_transfer() {
//!     // Create two affinities
//!     let affinities = create_manual_pinned_affinities(&[2]);
//!
//!     // Create an instance of MyData
//!     let data = MyData { value: 42 };
//!
//!     // Transfer data from one affinity to another
//!     let transferred_data = data.relocated(affinities[0].into(), affinities[1]);
//!
//!     // Use Inert to create a type that does not transfer data
//!     struct MyInertData(i32);
//!
//!     let inert_data = Unaware(MyInertData(100));
//!     let transferred_inert_data = inert_data.relocated(affinities[0].into(), affinities[1]);
//! }
//! ```
//!
//! ## Derive Macro Example
//!
//! When the `derive` feature (enabled by default) is active you can simply
//! derive [`ThreadAware`] instead of writing the implementation manually.
//!
//! ```rust
//! use thread_aware::{ThreadAware, create_manual_pinned_affinities};
//!
//! #[derive(Debug, Clone, ThreadAware)]
//! struct Point {
//!     x: i32,
//!     y: i32,
//! }
//!
//! fn derived_example() {
//!     let affinities = create_manual_pinned_affinities(&[2]);
//!     let p = Point { x: 5, y: 9 };
//!     // Transfer the value between two affinities. In this simple case the
//!     // data just gets copied, but for complex types the generated impl
//!     // calls `transfer` on each field.
//!     let _p2 = p.relocated(affinities[0].into(), affinities[1]);
//! }
//! ```
//!
//! If you disable default features (or the `derive` feature explicitly) you
//! can still implement [`ThreadAware`] manually as shown in the earlier example.

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware/favicon.ico")]

mod affinity;
mod cell;
mod closure;
pub mod core;
mod impls;
mod wrappers;

#[cfg(feature = "threads")]
#[cfg_attr(docsrs, doc(cfg(feature = "threads")))]
mod validator;

#[cfg(feature = "threads")]
#[cfg_attr(docsrs, doc(cfg(feature = "threads")))]
mod registry;

pub use core::{ThreadAware, create_manual_memory_affinities, create_manual_pinned_affinities};

pub use affinity::{MemoryAffinity, PinnedAffinity};

// Re-export the derive macro (behind the `derive` feature) so users can
// simply `use thread_aware::ThreadAware;`. Disable the feature to avoid the
// proc-macro dependency in minimal builds.

/// Derive macro implementing `ThreadAware` for structs and enums.
///
/// The generated implementation transfers each field by calling its own
/// `ThreadAware::relocated` method. Fields annotated with `#[thread_aware(skip)]` are
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
/// use thread_aware::{PinnedAffinity, MemoryAffinity, ThreadAware};
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
/// fn demo(mut a1: MemoryAffinity, mut a2: PinnedAffinity, w: Wrapper) -> Wrapper {
///     // Move the wrapper from a1 to a2.
///     let moved = w.relocated(a1.clone(), a2.clone().into());
///     moved
/// }
/// ```
#[cfg(feature = "derive")]
#[cfg_attr(docsrs, doc(cfg(feature = "derive")))]
pub use ::thread_aware_macros::ThreadAware;
pub use cell::{Arc, PerAppStorage, PerCore, PerCoreStorage, PerNuma, PerNumaStorage, PerProcess, Storage, Strategy};
pub use closure::{Closure, ClosureMut, ClosureOnce, RelocateFn, RelocateFnMut, RelocateFnOnce, relocate, relocate_mut, relocate_once};
#[cfg(feature = "threads")]
#[cfg_attr(docsrs, doc(cfg(feature = "threads")))]
pub use registry::{ProcessorCount, ThreadRegistry};
#[cfg(feature = "threads")]
#[cfg_attr(docsrs, doc(cfg(feature = "threads")))]
pub use validator::ThreadAwareValidator;
pub use wrappers::{Unaware, unaware};
