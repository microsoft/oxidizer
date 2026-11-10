// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(
    not(feature = "std"),
    expect(rustdoc::broken_intra_doc_links, reason = "all-features documentation validates std links")
)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_core/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_core/favicon.ico"
)]

//! Core types for values that adapt after a move to another thread.
//!
//! This crate provides the shared API for thread-aware libraries:
//!
//! - [`ThreadAware`] is the trait for values that adapt after a move.
//! - [`Thread`] describes the runtime, OS thread, and nearest memory for a value.
//!
//! [`Thread`] is a coordinate, not a handle. It owns no operating-system resource.
//! [`std::thread::Thread`] is a separate handle to a live OS thread. Use an alias if a module
//! needs both types.
//!
//! # The `thread_aware` family
//!
//! Use this crate for the stable types that public APIs expose. It has no derive macros, closure
//! adapters, policy wrappers, or runtime integration.
//!
//! The [`thread_aware`] crate provides those utilities. It includes a
//! [`#[derive(ThreadAware)]`][derive] macro, closure adapters, wrappers for foreign types, and
//! runtime coordinate construction. Its [authoring guide] explains how to make a type
//! thread-aware. The `performables` crate provides strategy-partitioned shared state.
//!
//! [`thread_aware`]: https://docs.rs/thread_aware
//! [derive]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
//! [arc]: https://docs.rs/performables/latest/performables/arc/struct.Arc.html
//! [authoring guide]: https://docs.rs/thread_aware/latest/thread_aware/documentation/index.html
//!
//! Depend on this crate directly if you only need the stable types. It has no normal
//! dependencies. Without the default `std` feature, [`Thread`] keeps [`Owner`] and [`NumaNode`]
//! but has no thread id.
//!
//! # Why relocation exists
//!
//! A runtime can keep memory and resources close to each worker. A moved value can still refer to
//! memory or resources from its old worker. [`ThreadAware`] lets the value adapt this state.
//! The runtime moves the value, then calls [`relocate`](ThreadAware::relocate) on its new thread.
//!
//! # Library authors: implementing the trait
//!
//! Library and application authors implement [`ThreadAware`]. They usually use the
//! [`#[derive(ThreadAware)]`][derive] macro. A runtime constructs [`Thread`] values and calls
//! [`relocate`](ThreadAware::relocate). The method is a callback like [`Drop::drop`].
//!
//! The derive macro is in [`thread_aware`]. A library can use that crate as an implementation
//! dependency while its public API uses types from this crate:
//!
//! ```ignore
//! // An implementation dependency, not part of the public API.
//! use thread_aware::ThreadAware;
//!
//! /// A codec with a scratch buffer that follows its nearest memory.
//! #[derive(ThreadAware)]
//! pub struct Encoder {
//!     scratch: Scratch,
//!     dictionary: Dictionary,
//! }
//! ```
//!
//! The derive forwards `relocate` to `scratch` and `dictionary`. Callers of `Encoder` do not need
//! to name [`thread_aware`].
//!
//! # Runtime authors: driving relocation
//!
//! Runtime authors create [`Thread`] values for their workers. After a runtime moves a value, the
//! destination thread calls [`relocate`](ThreadAware::relocate).
//!
//! `destination` describes the thread that runs the callback. It does not request work on another
//! thread. This order lets the callback release or create state with the destination thread's
//! memory locality.
//!
//! # Performance, not correctness
//!
//! Relocation is an optimization. A value must remain correct if the runtime never calls
//! `relocate`, calls it more than once, or gives it equal coordinates. A missed call can reduce
//! performance, but it must not cause a panic, deadlock, or wrong result.
//!
//! The runtime controls call frequency. Implementations must keep calls with unchanged relevant
//! coordinates cheap. They must also avoid contended synchronization and long blocking work.
//!
//! # What the ids mean
//!
//! - A thread id identifies a live OS thread.
//! - [`NumaNode`] identifies nearby memory. Runtimes can compare nodes only if they use the same
//!   numbering.
//! - [`Owner`] identifies the runtime that owns a [`Thread`].
//!
//! These ids are opaque and do not have to be consecutive. Use only the coordinate that the state
//! needs. Use a map instead of an indexed array for keyed state.
//!
//! # Relation to `Send`
//!
//! [`ThreadAware`] requires [`Send`]. The runtime first sends the value to its destination thread.
//! It then tells the value where it arrived. [`Send`] makes the move safe. [`ThreadAware`] adds
//! only the adaptation callback.
//!
//! # Provided implementations
//!
//! Values without thread-local state use an empty implementation. Containers forward relocation
//! to their values. Map keys stay unchanged.
//!
//! General references, sets, `Cow`, and `Arc` have no implementation. Their relocation policy is
//! ambiguous or can break their invariants. The exception is `&'static str`, which cannot dangle
//! and has no referent state to relocate. [`thread_aware`] provides wrappers for explicit policies.
//! The `performables` crate provides a strategy-partitioned [`Arc`][arc].
//!
//! # Features
//!
//! - `std` (default): Adds runtime construction support and [`Thread::id`]. It also implements
//!   [`ThreadAware`] for `HashMap`, `Path`, `PathBuf`, `SocketAddr`, `SocketAddrV4`, and
//!   `SocketAddrV6`.
//!
//! Without `std`, the crate needs `alloc` and pointer-width atomics.

extern crate alloc;
#[cfg(any(test, feature = "std"))]
extern crate std;

mod impls;
mod thread;
mod thread_aware;

#[doc(hidden)]
pub mod __private;

#[doc(inline)]
pub use thread::{NumaNode, Owner, Thread};
#[doc(inline)]
pub use thread_aware::ThreadAware;
