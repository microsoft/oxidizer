// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A guide to authoring thread-aware types.
//!
//! The crate-level docs explain *what* [`ThreadAware`](crate::ThreadAware) is and the relocation
//! contract it expresses. This guide is the companion *how-to*: how to make your own types
//! thread-aware correctly, which implementation to reach for, how to test and debug the result,
//! and the mistakes that compile cleanly yet quietly do nothing.
//!
//! It is written for authors who see `T: ThreadAware` in an API and need to satisfy it, and for
//! reviewers deciding whether a `#[derive(ThreadAware)]` or a `#[thread_aware(skip)]` is the right
//! call. The lessons in [Anti-patterns](#anti-patterns) are drawn from migrating a large
//! production service onto an Oxidizer-backed runtime.
//!
//! # Why thread-awareness exists
//!
//! Oxidizer runtimes are thread-per-core: each worker owns its slice of the machine, and shared
//! state that silently spans cores turns into cross-NUMA traffic and lock contention. A
//! thread-aware type is told, through [`relocate`](crate::ThreadAware::relocate), that it has just
//! moved from one worker to another, and is given the chance to *rebind* its affinity-bearing
//! state - reconnect to the destination's I/O scheduler, re-home an allocation in the local NUMA
//! node, or detach from memory it was sharing with the previous worker.
//!
//! Relocation is a **performance cooperation**, never a correctness guarantee. A type must remain
//! correct if `relocate` is called at the wrong moment, called with the wrong threads, or never
//! called at all - see [Performance vs. Correctness](crate#performance-vs-correctness). That
//! single fact drives most of the guidance below: because nothing enforces relocation, a type that
//! *silently* fails to relocate is the failure mode to design against.
//!
//! # Authoring a thread-aware type
//!
//! ## Prefer the derive
//!
//! In almost all cases, implement [`ThreadAware`](crate::ThreadAware) with the derive macro. It
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
//! // A runtime hands `relocate` the worker the value came from and the one it is moving to.
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
//! Annotate a field with `#[thread_aware(skip)]` when it carries no affinity and should be moved
//! as-is: a plain identifier, a length, a foreign handle that does no thread-local work. A skipped
//! field is never relocated, and the derive adds a `where Self: Send` bound to keep the
//! `ThreadAware: Send` supertrait satisfied.
//!
//! ```rust
//! use thread_aware::ThreadAware;
//!
//! #[derive(ThreadAware)]
//! struct Request {
//!     body: Vec<u8>,
//!     // A request id has no thread affinity; moving it verbatim is correct.
//!     #[thread_aware(skip)]
//!     id: u64,
//! }
//! ```
//!
//! `skip` is a claim that a field genuinely has nothing to rebind. It is not an escape hatch for
//! "this field does not implement `ThreadAware` yet" - reach for [`Unaware`](crate::Unaware) or
//! [`Arc`](crate::Arc) for that, so the intent is visible in the type.
//!
//! ## What the generated bounds mean
//!
//! The derive bounds the **field type**, not the parameters inside it. For every relocated field
//! whose type mentions a generic parameter, it emits `where <field type>: ThreadAware` - the exact
//! obligation the generated body discharges when it relocates that field. So a `Vec<T>` field
//! yields `where Vec<T>: ThreadAware`, and a `Wrapper<T>` field yields
//! `where Wrapper<T>: ThreadAware`, governed by that wrapper's own impl rather than by a bound on
//! `T`. A field whose
//! type reaches no parameter, and a marker payload behind a function pointer
//! (`PhantomData<fn(*const T)>`), owe no bound at all. See
//! [the derive's reference](crate::ThreadAware#generic-bounds) for the full rules.
//!
//! ## Implementing the trait by hand
//!
//! Write the impl yourself when relocation means something specific - re-homing an allocation,
//! swapping a per-core cache, reconnecting to a scheduler. The method receives the source worker
//! (`None` if unknown) and the destination:
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
//!         // re-allocates in the destination's NUMA node instead of reaching across.
//!         self.buffer = Vec::new();
//!     }
//! }
//! ```
//!
//! # Choosing an implementation
//!
//! | You have… | Reach for | Because |
//! |---|---|---|
//! | A compound of thread-aware fields | `#[derive(ThreadAware)]` | Forwards relocation to each field. |
//! | A field with genuine per-core behavior | a hand-written impl | Only you know what "rebind" means. |
//! | A foreign type that carries no affinity | [`Unaware<T>`](crate::Unaware) | A `MoveAsIs<T>`: implements the trait as a no-op. |
//! | Shared state that should differ per worker | [`Arc<T, PerThread>`](crate::Arc) | Materializes a separate `T` per destination. |
//! | Shared state that is the same everywhere | [`Arc<T, PerProcess>`](crate::Arc) | Behaves as a vanilla `Arc`. |
//!
//! [`Unaware`](crate::Unaware) wraps a value and satisfies `ThreadAware` without reacting to
//! relocation - use it for inert, foreign, or allocation-free values that legitimately do not care
//! which worker they are on. Wrapping a type that *does* implement the trait is discouraged: it
//! silences that type's own relocation (a performance loss, not a correctness bug).
//!
//! The strategy-partitioned [`Arc`](crate::Arc) is the usual bridge to a type that does not
//! implement the trait itself: an `Arc<Foo, PerThread>` gives each worker its own `Foo`, while an
//! `Arc<Foo, PerProcess>` shares one - the same `Arc` API, differing only in what relocation does.
//!
//! # Anti-patterns
//!
//! These are the shapes that compile, satisfy `T: ThreadAware`, and still leave state stranded on
//! the wrong worker. None of them produces a compile error, and most produce no runtime warning
//! either, so they are worth recognizing by sight.
//!
//! ## `Clone` does not relocate
//!
//! This is the one to internalize first. A thread-aware type stores its affinity in a field that
//! only [`relocate`](crate::ThreadAware::relocate) mutates. **`Clone` copies that stored affinity
//! verbatim.** Cloning a value that was built on worker A and using the clone on worker B does not
//! move it to B - it is still bound to A, quietly, until something calls `relocate`.
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
//! their constraints, and at least one such marking per audit tends to be a compile-shortcut that
//! reduces to a silent no-op. When you take a dependency on a type being thread-aware, verify that
//! its relocation actually reaches the state you care about rather than inheriting the annotation as
//! fact.
//!
//! ## Relocate the whole graph once, at the boundary
//!
//! When work crosses into a worker from the outside - an FFI entry, a hand-off from a foreign
//! thread - relocate the entire long-lived dependency graph **once**, at that boundary, rather than
//! special-casing each affinity-bearing dependency downstream. Make the graph's root `ThreadAware`
//! (by
//! derive) so a single `relocate` at the entry rebinds every affinity-bearing resource beneath it.
//! Relocating a subtree while its parent was built from a stale clone (see above) is how affinity
//! goes stale in practice.
//!
//! # Testing
//!
//! Because relocation is silent when it is wrong, test it by observation, not by trusting that the
//! derive did the right thing. The reliable pattern is a leaf type whose `relocate` records that it
//! was called, composed into the type under test; after one relocation, assert that every
//! non-skipped field was reached and every skipped field was not.
//!
//! ```rust
//! use thread_aware::{Thread, ThreadAware};
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
//! fn assert_reaches_the_right_fields(from: Option<&Thread>, to: &Thread) {
//!     let mut value = UnderTest {
//!         tracked: Tracker::default(),
//!         skipped: Tracker::default(),
//!     };
//!     value.relocate(from, to);
//!     assert_eq!(value.tracked.relocations, 1, "non-skipped fields must be relocated");
//!     assert_eq!(value.skipped.relocations, 0, "skipped fields must not be relocated");
//! }
//! ```
//!
//! Construct the [`Thread`](crate::Thread) values a real test needs with
//! [`ThreadBuilder`](crate::ThreadBuilder) (available with the default `std` feature). The
//! `test-utils` feature additionally offers a [`Relocator`](crate::Relocator) helper for driving
//! relocations in tests.
//!
//! # Debugging and telemetry
//!
//! When a value seems bound to the wrong worker, the question is almost always *was `relocate`
//! called, and did it reach this field?* The `Tracker` pattern above answers it in a test; in a
//! running system, a runtime that detects affinity-bearing state being touched from the wrong
//! worker is the signal to watch - for example, a debug-build warning such as a `*.thread_mismatch`
//! event with a backtrace at the offending access. Treat such a warning as a missing or too-late
//! `relocate`, most often a [stale clone](#clone-does-not-relocate).
//!
//! Remember that the *absence* of a warning does not prove correctness: relocation is best-effort,
//! so a value can be on the wrong worker with no diagnostic at all. Coverage of the relocation path
//! belongs in your tests, not in production telemetry.
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
