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
//! **Runtime authors** construct a [`Thread`] per worker and call
//! [`relocate`](ThreadAware::relocate) after moving a value, passing where it came from and
//! where it now runs. The example below plays the part of the runtime so the order is
//! visible.
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
//! let owner = Owner::new(2);
//! let first = Thread::new(owner.clone(), here, NumaNode::new(0));
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
//! - **Thread id** — a `std::thread::ThreadId`, unique among the threads alive at once, so
//!   state keyed on it is never shared by accident, not even between two runtimes in one
//!   process.
//! - **[`NumaNode`]** — the memory closest to that thread. Unlike the thread id it is
//!   *shared*: every thread near the same memory reports the same node, which is what suits
//!   it to state shared within a region but not across the machine. That holds only while
//!   every runtime numbers the regions identically. Nothing checks it, and runtimes that
//!   disagree make shared state wrong rather than merely slow.
//! - **[`Owner`]** — the runtime a [`Thread`] belongs to. Every new owner is unique, so two
//!   live runtimes never share one. It lets a value detect that it has crossed into a
//!   different runtime and release anything the previous one owned.
//!
//! An implementation reads only the ids its state depends on. A per-thread cache or a handle
//! to a thread-local driver keys on the thread id; a buffer pool keys on [`NumaNode`] and
//! survives a move to another thread near the same memory; anything the runtime owns compares
//! [`Owner`].
//!
//! The ids carry no meaning beyond identity: they need not start at zero or run
//! consecutively, and the [`Thread`]s in use cannot be enumerated. State keyed on any of them
//! belongs in a map rather than an array indexed by it. [`Owner::min_threads`] is the one
//! number on offer, and it is a floor to pre-size against, not a bound to index against.
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
//! `Vec`, `VecDeque`, `Box`, cells, tuples of up to twelve elements, and map values.
//!
//! References are not [`ThreadAware`]. Relocating through one would adapt something the value
//! only borrows, and whoever owns it is relocated on its own account.
//!
//! Map keys are left alone, since altering one could change its hash or ordering and corrupt
//! the map. Sets are not implemented at all for the same reason, so a `HashSet` or
//! `BTreeSet` field is not [`ThreadAware`].
//!
//! `Cow` is omitted for now: relocating a borrowed one has to clone it into owned storage
//! first, which is a surprising amount of work to hide behind a hint.
//!
//! `Arc` is also omitted: whether a shared allocation should stay shared across threads or
//! be split per thread depends on what it holds. The per-core [`Arc`][arc] in
//! [`thread_aware`] covers the case where splitting is correct.
//!
//! # Features
//!
//! - **`std`** *(default)* - Adds [`Thread::new`] and [`Thread::id`], which need
//!   [`ThreadId`](std::thread::ThreadId), and implements [`ThreadAware`] for standard library
//!   types such as `HashMap`, `Path` and `PathBuf`. Turn it off for `no_std`, which needs
//!   only `alloc` and pointer-width atomics.

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
