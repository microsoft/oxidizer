// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A guide to authoring thread-aware types.
//!
//! The crate-level docs explain *what* [`ThreadAware`](crate::ThreadAware) is and the relocation
//! contract it expresses. This guide is the companion *how-to*: how to make your own types
//! thread-aware correctly, which implementation to reach for, how to test the result, and the
//! mistakes that compile cleanly yet quietly do nothing.
//!
//! It is written for authors who see `T: ThreadAware` in an API and need to satisfy it, and for
//! reviewers deciding whether a `#[derive(ThreadAware)]` or a `#[thread_aware(skip)]` is the right
//! call. The lessons in [Anti-patterns](#anti-patterns) are drawn from migrating a large
//! production service onto an Oxidizer-backed runtime.
//!
//! # Why thread-awareness exists
//!
//! The crate-level [Theory of Operation](crate#theory-of-operation) covers what relocation is and
//! why thread-per-core runtimes need it. The one idea this guide leans on: relocation is a
//! **performance cooperation, never a correctness guarantee** (see
//! [Performance vs. Correctness](crate#performance-vs-correctness)). Nothing enforces it, so the
//! failure mode to design against is a type that *silently* fails to relocate.
//!
//! # Authoring a thread-aware type
//!
//! ## Prefer the derive
//!
//! In almost all cases, implement [`ThreadAware`](crate::ThreadAware) with
//! [the derive macro](derive@crate::ThreadAware). It
//! generates a [`relocate`](crate::ThreadAware::relocate) that forwards the notification to every
//! field, which is exactly what a compound type owes its parts:
//!
//! ```rust
//! use thread_aware::{Thread, ThreadAware};
//!
//! #[derive(ThreadAware)]
//! struct Connection {
//!     pool: Vec<u8>,
//!     scratch: String,
//! }
//!
//! // The runtime calls this on the worker that now owns `c`; `to` describes that current thread.
//! fn on_move(mut c: Connection, from: Option<&Thread>, to: &Thread) {
//!     c.relocate(from, to);
//! }
//! ```
//!
//! The `std` library types you are most likely to hold - `Vec`, `Box`, `Option`, `Result`, tuples,
//! arrays, maps - already implement the trait, so the derive "just works" on compounds of them.
//!
//! ## Skipping a field
//!
//! Reach for `#[thread_aware(skip)]` when a field's type does not implement `ThreadAware` and has
//! no affinity to rebind - a foreign handle, an FFI resource that does no thread-local work. The
//! field is never relocated, and the derive drops the `ThreadAware` bound it would otherwise place
//! on it (adding `where Self: Send` to keep the supertrait satisfied).
//!
//! ```rust
//! use thread_aware::ThreadAware;
//!
//! // A handle from a C library: it does not implement `ThreadAware`, and it carries no
//! // thread affinity of its own.
//! struct ForeignHandle {
//!     raw: usize,
//! }
//!
//! #[derive(ThreadAware)]
//! struct Request {
//!     body: Vec<u8>,
//!     #[thread_aware(skip)]
//!     handle: ForeignHandle,
//! }
//! ```
//!
//! Do not skip a field whose type already implements `ThreadAware` - even a primitive like `u64`,
//! whose impl is a harmless no-op. Forwarding is free and stays correct if the field later gains
//! affinity-bearing state; `skip` removes that safety net, so revisit each `skip` whenever the
//! field type changes. When you instead want a non-`ThreadAware` value to read as explicitly inert
//! in the type, wrap it in [`Unaware`](crate::Unaware) rather than skipping.
//!
//! ## What the generated bounds mean
//!
//! You rarely need to reason about this: the derive adds exactly the `ThreadAware` bounds its
//! generated body needs and no more, so a correct type "just derives". When it matters - a generic
//! wrapper, or a marker field that should stay bound-free - the derive's
//! [Generic Bounds](derive@crate::ThreadAware#generic-bounds)
//! reference has the rules.
//!
//! ## Implementing the trait by hand
//!
//! Write the impl yourself when relocation means something specific - re-homing an allocation,
//! swapping a per-core cache, reconnecting to a scheduler. The method runs on the destination
//! worker and receives the source worker (`None` if unknown) plus a coordinate describing the
//! current worker. The destination is descriptive; it is not a request to run the callback on
//! another worker:
//!
//! ```rust
//! use thread_aware::{Thread, ThreadAware};
//!
//! struct PerCoreScratch {
//!     buffer: Vec<u8>,
//! }
//!
//! impl ThreadAware for PerCoreScratch {
//!     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
//!         // The scratch buffer belonged to the previous worker; drop it so the next use
//!         // re-allocates fresh on the destination worker, letting its allocator place it in
//!         // local memory instead of carrying the old worker's buffer across.
//!         self.buffer = Vec::new();
//!     }
//! }
//! ```
//!
//! [`relocate`](crate::ThreadAware::relocate) has no error channel and must not panic. Because it
//! runs on the runtime's placement path, it also must not perform long or external blocking work -
//! no contended lock, network or disk I/O, or waiting on external progress (brief in-memory
//! coordination is fine). If the ideal adaptation is unavailable (a reconnect fails, a resource
//! can't be rebuilt), keep the existing usable state, defer the work, or fall back to a slower path
//! rather than unwinding; see the [trait contract](crate::ThreadAware::relocate) for the full
//! requirements.
//!
//! ## Per-worker state with `Arc`
//!
//! When several workers share a value but each should keep its *own* instance - a per-core cache, a
//! pool you do not want contended across cores - reach for the strategy-partitioned `Arc<T, S>` from
//! the separate [`performables`](https://docs.rs/performables) crate
//! ([`performables::arc::Arc`](https://docs.rs/performables/latest/performables/arc/struct.Arc.html);
//! add it as a dependency). With the
//! [`PerThread`](https://docs.rs/performables/latest/performables/arc/struct.PerThread.html)
//! strategy, each worker keeps its own value instead of sharing one process-wide: a worker that
//! already has one reuses it, and a worker that does not is given one when it is relocated there
//! (built from the pointer's factory, if it was constructed with one) *during* the `relocate` call,
//! not lazily on first use. Use
//! [`PerProcess`](https://docs.rs/performables/latest/performables/arc/struct.PerProcess.html), which
//! behaves as a vanilla `Arc`, when one shared instance is what you want, and
//! [`PerNuma`](https://docs.rs/performables/latest/performables/arc/struct.PerNuma.html) for one
//! instance per NUMA node. This is also the usual bridge to a type that does not implement
//! `ThreadAware` itself.
//!
//! # Choosing an implementation
//!
//! | You have… | Reach for | Because |
//! |---|---|---|
//! | A compound of thread-aware fields | `#[derive(ThreadAware)]` | Forwards relocation to each field. |
//! | A field with genuine per-core behavior | a hand-written impl | Only you know what "rebind" means. |
//! | A foreign type that carries no affinity | [`Unaware<T>`](crate::Unaware) | Implements relocation as a no-op; moves the wrapped value unchanged. |
//! | Shared state that should differ per worker | [`performables::arc::Arc<T, PerThread>`](https://docs.rs/performables/latest/performables/arc/struct.Arc.html) | Gives each worker its own `T`. |
//! | Shared state that is the same everywhere | [`performables::arc::Arc<T, PerProcess>`](https://docs.rs/performables/latest/performables/arc/struct.Arc.html) | Behaves as a vanilla `Arc`. |
//!
//! [`Unaware`](crate::Unaware) wraps a value
//! and satisfies `ThreadAware` without reacting to
//! relocation - use it for inert, foreign, or allocation-free values that legitimately do not care
//! which worker they are on. Wrapping a type that *does* implement the trait is discouraged: it
//! silences that type's own relocation (a performance loss, not a correctness bug).
//!
//! # Anti-patterns
//!
//! These are the shapes that compile, satisfy `T: ThreadAware`, and still leave state stranded on
//! the wrong worker. None of them produces a compile error, and most produce no runtime warning
//! either, so they are worth recognizing by sight.
//!
//! ## `Clone` does not relocate
//!
//! This is the one to internalize first. A thread-aware type typically stores its affinity in a
//! field that only [`relocate`](crate::ThreadAware::relocate) mutates - and a derived `Clone` then
//! **copies that stored affinity verbatim**. A hand-written `Clone` could rebind instead, but the
//! trait neither requires nor guarantees that. Cloning such a value built on worker A and using the
//! clone on worker B does not move it to B - it stays bound to A, quietly, until something calls
//! `relocate`.
//!
//! ```text
//! let services = build_on_startup_worker();     // affinity = startup worker
//! let per_request = services.clone();           // affinity = startup worker (copied!)
//! // `per_request` now funnels every task back onto the startup worker.
//! ```
//!
//! In one migration this single clone routed an entire process's work onto one core while the
//! others idled. If you clone a long-lived, affinity-bearing graph, relocate the clone at the point
//! it enters its new worker.
//!
//! ## `skip` on the only field is a silent no-op
//!
//! `#[thread_aware(skip)]` on the *sole* field of a type makes `relocate` do nothing, yet the type
//! still satisfies `T: ThreadAware`. Downstream code compiles, runtimes accept it, and no affinity
//! ever moves - with no error and no warning. A type whose every field is skipped is
//! indistinguishable from one that is genuinely inert; make sure that is what you meant.
//!
//! ## Do not trust inherited markings
//!
//! An existing `#[derive(ThreadAware)]` or `#[thread_aware(skip)]` is a decision someone made under
//! their constraints, and a marking can reduce to a silent no-op. When you take a dependency on a
//! type being thread-aware, verify that its relocation actually reaches the state you care about
//! rather than inheriting the annotation as fact.
//!
//! ## Relocate the whole graph once, at the boundary
//!
//! When work crosses into a worker from the outside - an FFI entry, a hand-off from a foreign
//! thread - relocate the entire long-lived dependency graph **once**, at that boundary, rather than
//! special-casing each affinity-bearing dependency downstream. Make the graph's root `ThreadAware`
//! (by derive) so a single `relocate` at the entry rebinds every affinity-bearing resource beneath
//! it.
//! Relocating a subtree while its parent was built from a stale clone (see above) is how affinity
//! goes stale in practice.
//!
//! # Testing
//!
//! Relocation is silent when it is wrong, so test it by observation. Compose a leaf type whose
//! `relocate` records that it ran, relocate the type under test once, and assert that every
//! non-skipped field was reached and every skipped one was not:
//!
//! ```rust
//! # fn main() {
//! # #[cfg(feature = "std")] {
//! use std::thread;
//!
//! use thread_aware::{Thread, ThreadAware, ThreadBuilder};
//!
//! /// Counts relocations so a test can prove which fields the derive reaches.
//! #[derive(Default)]
//! struct Tracker {
//!     relocations: usize,
//! }
//!
//! impl ThreadAware for Tracker {
//!     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {
//!         self.relocations += 1;
//!     }
//! }
//!
//! #[derive(ThreadAware)]
//! struct UnderTest {
//!     tracked: Tracker,
//!     #[thread_aware(skip)]
//!     skipped: Tracker,
//! }
//!
//! // Build two worker coordinates and relocate the value between them.
//! let builder = ThreadBuilder::default();
//! let from = builder.build(thread::current().id());
//! let other = thread::spawn(|| thread::current().id()).join().unwrap();
//! let to = builder.build(other);
//!
//! let mut value = UnderTest {
//!     tracked: Tracker::default(),
//!     skipped: Tracker::default(),
//! };
//! value.relocate(Some(&from), &to);
//!
//! assert_eq!(
//!     value.tracked.relocations, 1,
//!     "non-skipped fields must be relocated"
//! );
//! assert_eq!(
//!     value.skipped.relocations, 0,
//!     "skipped fields must not be relocated"
//! );
//! # }
//! # }
//! ```
//!
//! The example runs its own assertions, so removing the `relocate` call or changing either count
//! makes it fail. Building [`Thread`](crate::Thread) coordinates this way needs the `std` feature;
//! for a real test suite the `test-utils` feature's
//! [`Relocator`](https://docs.rs/thread_aware/latest/thread_aware/struct.Relocator.html) (which
//! implies `std`) drives relocations without hand-built coordinates.
//!
//! `UnderTest` here only exercises the derive's field-forwarding mechanics. Point the same
//! observe-relocation technique at your *real* type: instantiate it with a recording leaf where it
//! is generic or dependency-injected, otherwise capture its real affinity-bearing state before and
//! after `relocate`. A green test on a stand-in proxy does not prove your production graph
//! relocates.
//!
//! # Validating correctness
//!
//! What the toolchain checks for you, and what it cannot:
//!
//! * **The compiler** enforces the `ThreadAware: Send` supertrait and, through the derive's
//!   field-type bounds, that every relocated field is itself thread-aware. It cannot tell whether a
//!   `#[thread_aware(skip)]` is *justified* - only that the resulting type is still `Send`.
//! * **The derive** emits each field-type predicate once and suppresses a bound the author already
//!   wrote, so a correct `#[derive(ThreadAware)]` does not trip
//!   `clippy::trait_duplication_in_bounds`.
//! * **Your tests** are the only thing that checks the property that actually matters: that
//!   relocation reaches the state it is supposed to. Nothing else does.
//!
//! The through-line of this guide: a thread-aware type that does the wrong thing usually does it
//! silently. Author for that - prefer the derive, justify every `skip`, relocate clones and graphs
//! at their boundaries, and prove it with a test that observes relocation happening.
