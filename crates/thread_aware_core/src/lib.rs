// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_core/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/thread_aware_core/favicon.ico"
)]

//! Support for values that adapt when a runtime moves them to another thread.
//!
//! This crate contains the small API shared by thread-aware libraries:
//!
//! - [`ThreadAware`] notifies a value that it has moved.
//! - [`Thread`] records where it now runs: which runtime, which OS thread, and which memory is
//!   closest to it.
//!
//! [`Thread`] is a coordinate, not a handle: a runtime builds one to describe where a value
//! is running, and it owns no operating-system resource. It is unrelated to
//! [`std::thread::Thread`], which is a handle to a live OS thread. Naming both in one module
//! requires aliasing one of them.
//!
//! # The `thread_aware` family
//!
//! - **`thread_aware_core`** (this crate) — the vocabulary that two unrelated libraries must
//!   agree on before either can relocate a value defined by the other. Deliberately small and
//!   slow-moving, so naming [`ThreadAware`] or [`Thread`] in your own public API costs you
//!   nothing later.
//! - **[`thread_aware`]** — the utilities that make relocation convenient: a
//!   [`#[derive(ThreadAware)]`][derive] macro, wrappers for foreign types, a per-core
//!   [`Arc`][arc], containers and registries. Free to evolve, and not meant to appear in a
//!   public API.
//!
//! [`thread_aware`]: https://docs.rs/thread_aware
//! [derive]: https://docs.rs/thread_aware/latest/thread_aware/derive.ThreadAware.html
//! [arc]: https://docs.rs/thread_aware/latest/thread_aware/struct.Arc.html
//!
//! Depend on this crate directly when all you need is the trait. It adds nothing to your
//! dependency graph, and works without `std`: with default features turned off, [`Thread`]
//! loses its thread id component and keeps [`Owner`] and [`NumaNode`].
//!
//! # Why relocation exists
//!
//! Thread-per-core and NUMA-aware runtimes are fast because each worker keeps to itself: it
//! uses memory close to its own thread, talks to its own I/O driver, and does not
//! synchronize with other workers. When a value moves to another worker, what used to be
//! close by is now in the wrong place: a cache line shared between threads, memory in a
//! distant region, a handle to another thread's driver.
//!
//! [`ThreadAware`] lets that state repair itself. The runtime moves the value, then calls
//! [`relocate`](ThreadAware::relocate) to report where it now lives. Relocation has two
//! sides, and most code sits on only one of them.
//!
//! # Library authors: implementing the trait
//!
//! **Library and application authors** implement [`ThreadAware`], usually through the
//! [`#[derive(ThreadAware)]`][derive] macro. They never call
//! [`relocate`](ThreadAware::relocate) and never construct a [`Thread`]; the runtime does
//! both and then invokes the implementation. It is a callback, like [`Drop::drop`].
//!
//! The derive lives in [`thread_aware`], so a library that wants it depends on that crate.
//! Only the trait and [`Thread`] cross the public boundary, and both come from here, so the
//! dependency stays an implementation detail:
//!
//! ```ignore
//! // A build dependency, not part of what this library promises.
//! use thread_aware::ThreadAware;
//!
//! /// A codec whose scratch buffer should follow the memory it is used from.
//! #[derive(ThreadAware)]
//! pub struct Encoder {
//!     scratch: Scratch,
//!     dictionary: Dictionary,
//! }
//! ```
//!
//! The derive writes the forwarding implementation, calling `relocate` on `scratch` and
//! `dictionary` in turn. Because a composed type forwards to its fields, one call at the top
//! reaches everything below it. Callers of `Encoder` never name [`thread_aware`].
//!
//! # Runtime authors: driving relocation
//!
//! **Runtime authors** create a list of [`Thread`] values describing their workers. How that
//! list is constructed is a runtime implementation detail and is not relevant to runtime
//! consumers.
//!
//! After moving a value, the runtime calls [`relocate`](ThreadAware::relocate), passing where
//! the value came from and where it now runs.
//!
//! # Performance, not correctness
//!
//! Relocation is an optimization, not a guarantee. A value must remain correct if the call
//! never comes, comes twice, or reports the same source and destination. Missing calls are
//! normal: a value can reach another thread through `std::thread::spawn`, a channel, or a
//! runtime that knows nothing about this trait. That may make things slower, but it must
//! never cause a panic, a deadlock, or a wrong answer.
//!
//! Nor is it a hot path. Expect roughly one relocation per object graph per job or request,
//! after which the value is used normally. Avoiding synchronization matters more than saving a
//! few cycles.
//!
//! # What the ids mean
//!
//! - **Thread id** identifies a live OS thread.
//! - **[`NumaNode`]** identifies nearby memory and is shared by threads in the same region.
//!   It is meaningful across runtimes only when they number regions identically.
//! - **[`Owner`]** uniquely identifies the runtime a [`Thread`] belongs to.
//!
//! These ids are opaque and need not be consecutive. Use only the coordinate your state
//! depends on, and store keyed state in a map rather than an indexed array.
//!
//! # Relation to `Send`
//!
//! [`ThreadAware`] requires [`Send`], and in that order: a value is sent to another thread
//! first, then told where it landed. [`Send`] is what makes the move safe, and
//! [`ThreadAware`] adds nothing to it.
//!
//! # Provided implementations
//!
//! Values with no thread-local state use an empty implementation. Containers forward
//! relocation to their values, while map keys remain unchanged.
//!
//! References, sets, `Cow`, and `Arc` have no implementation because relocation would be
//! ambiguous or could violate their invariants. [`thread_aware`] provides wrappers for cases
//! that need an explicit policy, including its per-core [`Arc`][arc].
//!
//! # Features
//!
//! - **`std`** *(default)* - Adds runtime construction support and [`Thread::id`], which need
//!   [`ThreadId`](std::thread::ThreadId), and implements [`ThreadAware`] for standard library
//!   types such as `HashMap`, `Path` and `PathBuf`. Turn it off for `no_std`, which needs only
//!   `alloc` and pointer-width atomics.

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
