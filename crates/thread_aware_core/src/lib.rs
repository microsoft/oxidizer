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
//! - **`thread_aware`** — the utilities that make relocation convenient: a
//!   `#[derive(ThreadAware)]` macro, wrappers for foreign types, a per-core `Arc`, containers
//!   and registries. Free to evolve, and not meant to appear in a public API.
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
//! [`relocate`](ThreadAware::relocate) to report where it now lives.
//!
//! # The two roles
//!
//! **Library and application authors** implement [`ThreadAware`], usually through the
//! `#[derive(ThreadAware)]` macro. They never call [`relocate`](ThreadAware::relocate) and
//! never construct a [`Thread`]; the runtime does both and then invokes the implementation.
//! It is a callback, like [`Drop::drop`].
//!
//! The derive lives in `thread_aware`, so a library that wants it depends on that crate.
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
//!
//! impl Encoder {
//!     /// Public API: names only `thread_aware_core` types.
//!     pub fn describe(&self, running_on: &thread_aware_core::Thread) -> String {
//!         format!("encoding near {:?}", running_on.numa_node())
//!     }
//! }
//! ```
//!
//! The derive writes the forwarding implementation, calling `relocate` on `scratch` and
//! `dictionary` in turn. Callers of `Encoder` never name `thread_aware`.
//!
//! **Runtime authors** construct a [`Thread`] per worker and call
//! [`relocate`](ThreadAware::relocate) after moving a value, passing where it came from and
//! where it now runs.
//!
//! A type composed of other types forwards the call to its fields, so one call at the top
//! reaches everything below it. The derive macro and containers in `thread_aware` do this
//! automatically.
//!
//! The example below plays the part of the runtime so the order is visible.
//!
//! ```
//! # fn main() {
//! # #[cfg(feature = "std")] {
//! use std::thread;
//!
//! use thread_aware_core::{NumaNode, Owner, Thread, ThreadAware};
//!
//! // What a library author writes.
//! struct Worker {
//!     thread: Option<thread::ThreadId>,
//! }
//!
//! impl ThreadAware for Worker {
//!     fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
//!         self.thread = Some(destination.id());
//!     }
//! }
//!
//! // What the runtime does.
//! let here = thread::current().id();
//! let there = thread::spawn(|| thread::current().id()).join().unwrap();
//!
//! let owner = Owner::new(1);
//! let first = Thread::new(owner, here, NumaNode::new(0));
//! let second = Thread::new(owner, there, NumaNode::new(1));
//!
//! let mut worker = Worker { thread: None };
//!
//! worker.relocate(None, &first); // first placement, no previous `Thread`
//! worker.relocate(Some(&first), &second); // moved to another thread
//!
//! assert_eq!(worker.thread, Some(there));
//! # }
//! # }
//! ```
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
//! The thread id is `std::thread::ThreadId`. It identifies one thread and nothing else, and
//! is unique among the threads alive at the same time, so state keyed on it is never shared
//! by accident, not even between two runtimes in the same process.
//!
//! [`NumaNode`] identifies the memory closest to that thread. Unlike the thread id it is
//! shared: every thread near the same memory reports the same [`NumaNode`], which is what
//! makes it suitable for state shared within a region but not across the machine.
//!
//! That sharing holds only while every runtime in the process numbers the regions
//! identically, for example from the numbering the operating system reports. Nothing checks
//! it, and if two runtimes number them differently then state shared between them is wrong,
//! not merely slow. Share across runtimes only when all of them are under common control.
//!
//! [`Owner`] identifies the runtime that constructed a [`Thread`]. Thread ids already distinguish
//! threads, so this id answers a different question: it lets a value detect that it has
//! crossed into a different runtime and release anything the previous one owned.
//!
//! An implementation therefore reads only the ids its state depends on:
//!
//! - State that must not be shared at all, such as a per-thread cache or a handle to a
//!   thread-local driver, keys on the thread id and is replaced whenever the thread changes.
//! - State concerned only with memory locality, such as a buffer pool, keys on [`NumaNode`]
//!   and survives a move to another thread near the same memory.
//! - State owned by the runtime, such as a scheduler handle, also compares [`Owner`] and is
//!   released when it changes.
//!
//! The ids carry no meaning beyond identity. [`Owner`] and [`NumaNode`] need not start at
//! zero or run consecutively, no count is exposed, and the [`Thread`]s in use cannot be
//! enumerated. State keyed on any of these ids belongs in a map rather than an array indexed
//! by it.
//!
//! Without `std` there is no [`ThreadId`](std::thread::ThreadId): `Thread::new` and
//! `Thread::id` are absent and only [`Owner`] and [`NumaNode`] remain. A `no_std` library can
//! still implement [`ThreadAware`] and use whatever it is given; the runtime that drives
//! relocation requires `std` regardless.
//!
//! # Relation to `Send`
//!
//! [`ThreadAware`] requires [`Send`], and in that order: a value is sent to another thread
//! first, then told where it landed. [`Send`] is what makes the move safe, and
//! [`ThreadAware`] adds nothing to it.
//!
//! # Provided implementations
//!
//! Types with nothing tied to a thread receive an empty implementation: primitives and their
//! non-zero variants, the thread ids, `Duration`, strings, safe function pointers of up to
//! twelve parameters, and, with the `std` feature, paths.
//!
//! Containers forward the call to what they hold: [`Option`], [`Result`], arrays, slices,
//! `Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples of up to twelve elements, and map values.
//! A borrowed `Cow` is taken to owned so that it can be relocated as well.
//!
//! Map keys are left alone, since altering one could change its hash or ordering and corrupt
//! the map. Sets are not implemented at all for the same reason, so a `HashSet` or
//! `BTreeSet` field is not [`ThreadAware`].
//!
//! `Arc` is also omitted: whether a shared allocation should stay shared across threads or
//! be split per thread depends on what it holds. The per-core `Arc` in `thread_aware` covers
//! the case where splitting is correct.
//!
//! # Crate features
//!
//! * The **`std` Cargo feature** *(enabled by default)* provides [`Thread::new`] and
//!   [`Thread::id`], plus implementations for standard library types such as `HashMap`,
//!   `Path` and `PathBuf`. Turning it off yields a `no_std` build that requires only `alloc`.

extern crate alloc;
#[cfg(any(test, feature = "std"))]
extern crate std;

mod impls;
mod thread;
mod thread_aware;

#[doc(inline)]
pub use thread::{NumaNode, Owner, Thread};
#[doc(inline)]
pub use thread_aware::ThreadAware;
