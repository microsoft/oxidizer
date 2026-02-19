// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Essential building blocks for thread-per-core libraries.
//!
//! This crate allows you to express migrations between NUMA nodes, threads, or specific CPU cores.
//! It can serve as a foundation for building components and runtimes that operate across multiple
//! memory affinities.
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
//! ## Implementing [`ThreadAware`], and `Arc<T, PerCore>`
//!
//! In most cases [`ThreadAware`] should be implemented via the provided derive macro.
//! As thread-awareness of a type usually involves letting all contained fields know of an ongoing
//! relocation, the derive macro does just that. A default impl is provided for many `std` types,
//! so the macro should 'just work' on most compounds of built-ins.
//!
//! External crates might often not implement [`ThreadAware`]. In many of these cases using our
//! [`thread_aware::Arc`](Arc) offers a convenient solution: It combines an upstream
//! [`std::sync::Arc`] with a relocation [`Strategy`](storage::Strategy), and implements [`ThreadAware`] for it. For
//! example, while an `Arc<Foo, PerProcess>` effectively acts as vanilla `Arc`, an
//! `Arc<Foo, PerCore>` ensures a separate `Foo` is available any time the types moves a core boundary.
//!
//!
//! ## Relation to [`Send`]
//!
//! Although [`ThreadAware`] has no supertraits, any runtime invoking it will usually require the underlying type to
//! be [`Send`]. In these cases, type are first sent to another thread, then the [`ThreadAware`] relocation
//! notification is invoked.
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
//! [`ThreadAware`] is implemented for many standard library types, including primitive types, Vec,
//! String, Option, Result, tuples, etc. However, it's explicitly not implemented for [`std::sync::Arc`]
//! as that type implies some level of cross-thread sharing and thus needs special attention when used
//! from types that implement [`ThreadAware`].
//!
//! # Features
//!
//! * **`derive`** *(default)*: Re-exports the `#[derive(ThreadAware)]` macro from the companion
//!   `thread_aware_macros` crate. Disable to avoid pulling in proc-macro code in minimal
//!   environments: `default-features = false`.
//! * **`threads`**: Enables features mainly used by async runtimes for OS interactions.
//!
//! # Examples
//!
//! ## Deriving [`ThreadAware`]
//!
//! When the `derive` feature (enabled by default) is active you can simply
//! derive [`ThreadAware`] instead of writing the implementation manually.
//!
//! ```rust
//! use thread_aware::ThreadAware;
//!
//! #[derive(Debug, Clone, ThreadAware)]
//! struct Point {
//!     x: i32,
//!     y: i32,
//! }
//! ```
//!
//! ## Enabling [`ThreadAware`] via `Arc<T, S>`
//!
//! For types containing fields not [`ThreadAware`], you can use [`Arc`] to specify a
//! strategy, and wrap them in an [`Arc`] that implements the trait.
//!
//!
//! ```rust
//! use thread_aware::{Arc, PerCore, ThreadAware};
//! # #[derive(Debug, Default)]
//! # struct Client;
//!
//! #[derive(Debug, Clone, ThreadAware)]
//! struct Service {
//!     name: String,
//!     client: Arc<Client, PerCore>,
//! }
//!
//! impl Service {
//!     fn new() -> Self {
//!         Self {
//!             name: "MyService".to_string(),
//!             client: Arc::new(|| Client::default()),
//!         }
//!     }
//! }
//! ```

#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware/favicon.ico")]

mod cell;
mod core;
mod impls;
mod wrappers;

pub mod closure;

#[cfg(feature = "threads")]
pub mod registry;

#[doc(hidden)]
pub mod __private;
pub mod affinity;

#[doc(inline)]
pub use core::ThreadAware;

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
/// * `#[thread_aware(skip)]`: Prevents a field from being recursively transferred.
///
/// # Generic Bounds
/// Generic type parameters appearing in non-skipped fields automatically receive a
/// `::thread_aware::ThreadAware` bound (occurrences only inside `PhantomData<..>` are ignored).
///
/// # Example
/// ```rust
/// use thread_aware::ThreadAware;
/// use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};
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
pub use ::thread_aware_macros::ThreadAware;
pub use cell::{Arc, PerCore, PerNuma, PerProcess, storage};
pub use wrappers::{Unaware, unaware};
