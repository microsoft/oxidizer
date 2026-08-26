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
//! - [`Place`] records where it now runs: which runtime, which thread, and which memory is
//!   closest to it.
//!
//! The crate has no dependencies. It also works without `std`: with default features turned
//! off, [`Place`] loses its thread id and keeps [`Origin`] and [`NumaNode`]. The companion
//! `thread_aware` crate provides the conveniences on top: a `#[derive(ThreadAware)]` macro,
//! wrappers for foreign types, and a per-core `Arc`.
//!
//! # Why this crate is separate
//!
//! A crate that names a thread-aware type in its own public API inherits whatever that
//! type's crate promises. Keeping the trait and [`Place`] here, in something small,
//! dependency-free and slow-moving, lets such crates expose them without taking on the
//! larger surface. The containers, callbacks, registry and derive support in `thread_aware`
//! stay free to evolve, and are not meant to appear in a public API. Depend on this crate
//! directly when only the trait is needed.
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
//! never construct a [`Place`]; the runtime does both and then invokes the implementation.
//! It is a callback, like [`Drop::drop`].
//!
//! **Runtime authors** construct a [`Place`] per worker and call
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
//! use thread_aware_core::{NumaNode, Origin, Place, ThreadAware};
//!
//! // What a library author writes.
//! struct Worker {
//!     thread: Option<thread::ThreadId>,
//! }
//!
//! impl ThreadAware for Worker {
//!     fn relocate(&mut self, _source: Option<&Place>, destination: &Place) {
//!         self.thread = Some(destination.thread());
//!     }
//! }
//!
//! // What the runtime does.
//! let here = thread::current().id();
//! let there = thread::spawn(|| thread::current().id()).join().unwrap();
//!
//! let origin = Origin::from(1);
//! let first = Place::new(origin, here, NumaNode::from(0));
//! let second = Place::new(origin, there, NumaNode::from(1));
//!
//! let mut worker = Worker { thread: None };
//!
//! worker.relocate(None, &first); // first placement, no previous place
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
//! [`Origin`] identifies the runtime that produced the place. Thread ids already distinguish
//! threads, so this is a matter of ownership: it lets a value detect that it has crossed into
//! a different runtime and release anything the previous one owned.
//!
//! An implementation therefore reads only the ids its state depends on:
//!
//! - State that must not be shared at all, such as a per-thread cache or a handle to a
//!   thread-local driver, keys on the thread id and is replaced whenever the thread changes.
//! - State concerned only with memory locality, such as a buffer pool, keys on [`NumaNode`]
//!   and survives a move to another thread near the same memory.
//! - State owned by the runtime, such as a scheduler handle, also compares [`Origin`] and is
//!   released when it changes.
//!
//! The ids carry no meaning beyond identity. [`Origin`] and [`NumaNode`] need not start at
//! zero or run consecutively, no count is exposed, and the places in use cannot be
//! enumerated. Per-place state belongs in a map keyed by the id rather than an array indexed
//! by it.
//!
//! Without `std` there is no thread id: `Place::new` and `Place::thread` are absent and only
//! [`Origin`] and [`NumaNode`] remain. A `no_std` library can still implement
//! [`ThreadAware`] and use whatever it is given; the runtime that drives relocation requires
//! `std` regardless.
//!
//! # Relation to `Send`
//!
//! [`ThreadAware`] requires [`Send`], and in that order: a value is sent to another thread
//! first, then told where it landed. [`Send`] is what makes the move safe, and
//! [`ThreadAware`] adds nothing to it.
//!
//! # Provided implementations
//!
//! Types with nothing tied to a place receive an empty implementation: primitives and their
//! non-zero variants, the place ids, `Duration`, strings, safe function pointers of up to
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
//! * The **`std` Cargo feature** *(enabled by default)* provides the thread id half of
//!   [`Place`] and implementations for standard library types such as `HashMap`, `Path` and
//!   `PathBuf`. Turning it off yields a `no_std` build that requires only `alloc`.

extern crate alloc;
#[cfg(any(test, feature = "std"))]
extern crate std;

mod impls;
mod place;
mod thread_aware;

#[doc(inline)]
pub use place::{NumaNode, Origin, Place};
#[doc(inline)]
pub use thread_aware::ThreadAware;
