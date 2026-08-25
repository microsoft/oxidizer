// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]

//! Stable foundations for moving thread-isolated state between execution contexts.
//!
//! This crate contains the small API shared by thread-aware libraries:
//!
//! - [`ThreadAware`] notifies a value that it has moved to a different location.
//! - [`Location`] identifies the execution context — topology, core and memory
//!   region — that a value has moved to.
//!
//! The crate has no dependencies and is always `no_std`. Its opt-in `std`
//! feature adds implementations for standard-library types such as `HashMap`
//! and `&Path`.
//!
//! Ergonomics built on this foundation — a `#[derive(ThreadAware)]` macro, wrappers for
//! foreign types, and a per-core `Arc` — live in the companion `thread_aware` crate.
//! Depend on `thread_aware_core` directly when you only need to *implement* the trait,
//! so that your own public API does not pull in the larger crate.
//!
//! # Why relocation exists
//!
//! Thread-per-core and NUMA-aware runtimes get their performance from locality: a worker
//! touches memory in its own region, talks to its own I/O driver, and avoids synchronizing
//! with its peers. When a value moves from one worker to another, state that used to be
//! local becomes remote — a cache line now shared across cores, an allocation now on a
//! foreign NUMA node, a handle now pointing at another thread's driver.
//!
//! [`ThreadAware`] is the notification that lets such state repair itself. The runtime
//! moves the value, then calls [`relocate`](ThreadAware::relocate) to say *"you now live
//! here"*, and the value re-establishes whatever locality it cares about.
//!
//! # Theory of operation
//!
//! Two distinct roles share this trait, and it matters which one you are in.
//!
//! **Library and application authors — the common case.** You *implement* [`ThreadAware`]
//! on your types, usually through the `#[derive(ThreadAware)]` macro in the `thread_aware`
//! crate. You do **not** call [`relocate`](ThreadAware::relocate) yourself and you do not
//! construct a [`Location`]. A thread-aware runtime does that for you automatically
//! whenever it moves your value between workers; your implementation simply gets invoked.
//! Treat [`relocate`](ThreadAware::relocate) as a callback, in the same way you never call
//! [`Drop::drop`] by hand.
//!
//! **Runtime authors — the rare case.** A runtime establishes the topology, constructs the
//! [`Location`] values that describe its workers, and drives relocation by calling
//! [`relocate`](ThreadAware::relocate) after it has moved a value, passing the location the
//! value came from (when known) and the one it now runs on. Only code that owns the
//! placement of work needs to do this.
//!
//! Composite types forward the notification to their parts, so one call at the root of an
//! object graph reaches every field that cares. That is what the `thread_aware` derive
//! macro generates, and what the collection and container implementations in this crate do.
//!
//! The example below plays the part of the runtime explicitly so that the sequence is
//! visible. In real code the two `relocate` calls come from the runtime, not from you.
//!
//! ```
//! use thread_aware_core::{Core, Location, MemoryRegion, ThreadAware, Topology};
//!
//! // What a library author writes: an implementation, and nothing else.
//! struct Worker {
//!     core: Option<Core>,
//! }
//!
//! impl ThreadAware for Worker {
//!     fn relocate(&mut self, _source: Option<&Location>, destination: &Location) {
//!         self.core = Some(destination.core());
//!     }
//! }
//!
//! // What a runtime does on the library author's behalf.
//! let topology = Topology::from(1);
//! let first = Location::new(topology, Core::from(0), MemoryRegion::from(0));
//! let second = Location::new(topology, Core::from(3), MemoryRegion::from(1));
//!
//! let mut worker = Worker { core: None };
//!
//! // Initial placement; the previous location is unknown.
//! worker.relocate(None, &first);
//!
//! // Later, the runtime migrates the worker to another core.
//! worker.relocate(Some(&first), &second);
//!
//! assert_eq!(worker.core, Some(Core::from(3)));
//! ```
//!
//! # Performance, not correctness
//!
//! Relocation is a cooperative performance optimization rather than a correctness boundary.
//! Implementations must remain correct if a relocation notification is omitted, repeated,
//! or reports the same source and destination.
//!
//! Missed notifications are expected in practice: a value can reach another thread through
//! `std::thread::spawn`, a channel, or any runtime that does not participate in this
//! protocol. The only permitted consequence is degraded locality — never a panic, a
//! deadlock, or a wrong answer.
//!
//! Relocation is also not a hot path. The expected pattern is one relocation per object
//! graph per job or request — an incoming request is routed to a worker and its dependency
//! graph is relocated onto that worker — after which the steady-state hot path simply uses
//! the now-local state. Implementations should therefore favor avoiding synchronization
//! over shaving cycles.
//!
//! # Coordinate space
//!
//! [`Core`] and [`MemoryRegion`] are real hardware coordinates of the physical machine, not
//! dense indices into the worker list of one runtime. Their values are meaningful
//! process-wide: two runtimes on the same machine that both use core 2 report the same
//! [`Core`], and state keyed by it can legitimately be shared between them. Preserving that
//! sharing is why the API exposes identities rather than re-numbered indices.
//!
//! [`Topology`] identifies the runtime that produced the location. It does not scope the
//! hardware coordinates; it tells an implementation whether it is still inside the runtime
//! whose resources it holds.
//!
//! Which coordinates an implementation reads is its own choice:
//!
//! - Hardware-keyed state — a per-core cache, a region-local buffer pool — can key on
//!   [`Core`] or [`MemoryRegion`] alone and stay valid across topologies.
//! - Runtime-bound state — a task scheduler, a handle to a thread-local I/O driver — must
//!   also compare [`Topology`] and detach when it changes.
//!
//! The values themselves are opaque identities:
//!
//! - They are not promised to start at zero, to be contiguous, or to be bounded by the
//!   number of cores or regions in use. A machine may report cores `1` and `399` and
//!   nothing in between.
//! - No count is exposed, so the set of live locations is not enumerable through this API.
//!   Implementations that need per-location storage should key a map by the id rather than
//!   index into a pre-sized array. That is affordable because relocation is not a hot path:
//!   the lookup happens when a value moves, not on every use of the value.
//!
//! # Relation to `Send`
//!
//! [`ThreadAware`] requires [`Send`] as a supertrait, and the two happen in that order: a
//! value is first sent to another thread, and only then told where it landed. [`Send`]
//! remains the safety property; [`ThreadAware`] adds nothing to it.
//!
//! # Thread versus core semantics
//!
//! This crate targets thread-per-core runtimes, where each worker thread is pinned to one
//! logical processor, so "moved to another thread" and "moved to another core" describe the
//! same event. A runtime that runs several threads per core, or leaves threads unpinned, is
//! expected to surface that fact rather than pretend otherwise.
//!
//! # Provided implementations
//!
//! [`ThreadAware`] is implemented for types with no location-dependent state (primitives,
//! location identifiers, `Duration`, strings, function pointers, and, with the `std`
//! feature, paths). It is forwarded through container types such as [`Option`], [`Result`],
//! arrays, slices, `Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples up to twelve elements and
//! map values. Map keys and set elements are deliberately not relocated because changing
//! their equality, hashing or ordering would violate collection invariants.
//!
//! `Arc` is deliberately **not** implemented. Whether a shared allocation should stay shared
//! across cores or be split per core is a policy decision that depends on what the `Arc`
//! holds — read-mostly data is often fine to share — so no blanket implementation can make
//! it. The choice belongs to the type holding the `Arc`; the `thread_aware` crate provides a
//! per-core `Arc` for when splitting is the right answer.
//!
//! # Crate features
//!
//! * The **`std` Cargo feature** *(off by default)* adds implementations for
//!   standard-library types such as `HashMap`, `Path` and `PathBuf`. Without it the crate
//!   needs only `alloc`.

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod impls;
mod location;

#[doc(inline)]
pub use location::{Core, Location, MemoryRegion, Topology};

/// Notifies state that it has moved to a different [`Location`].
///
/// Implement this trait when part of a type is tied to *where* it runs: memory allocated in
/// a particular region, a handle to a thread-local driver, a shard index, or a cached core
/// id. [`relocate`](Self::relocate) is the hook where that state is brought back in line
/// with the new location.
///
/// [`relocate`](Self::relocate) is a callback, not something you call. A thread-aware
/// runtime invokes it for you when it moves your value between workers. Constructing
/// [`Location`] values and driving relocation is done by the runtime; see the
/// [theory of operation](crate#theory-of-operation) for the split between the two roles.
///
/// # When to implement
///
/// Implement or derive [`ThreadAware`] when your type may take part in an object graph that
/// a runtime relocates. That includes inert leaf types: an empty implementation is what lets
/// a containing type derive the trait. Do not implement it merely because a type is [`Send`],
/// and never move state that is required for *correctness* into
/// [`relocate`](Self::relocate) — it is only ever allowed to affect performance.
///
/// # Usage patterns
///
/// Most implementations take one of four shapes:
///
/// 1. **Do nothing.** The type holds no location-dependent state. An empty body is a
///    complete and correct implementation — this is what the built-in implementations for
///    primitives do.
///
///    ```
///    # use thread_aware_core::{Location, ThreadAware};
///    struct RequestId(u64);
///
///    impl ThreadAware for RequestId {
///        fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {}
///    }
///    ```
/// 2. **Record the destination.** Cache the new [`Core`] or [`MemoryRegion`] so that later
///    operations can route by it.
/// 3. **Re-acquire location-local resources.** Release the handle, pool or scratch buffer
///    obtained at the old location and acquire the equivalent one for the destination.
/// 4. **Forward to fields.** Composite types call `relocate` on each part. Prefer the
///    `#[derive(ThreadAware)]` macro from the `thread_aware` crate over hand-writing this.
///
/// # Contract
///
/// [`relocate`](Self::relocate) is best-effort and infallible. Implementations must:
///
/// * **Preserve logical state.** Relocation may rebuild caches, pools and handles — state
///   that exists only for speed. It must never discard or alter data the value's users can
///   observe.
/// * **Not fail.** There is no error channel. When the ideal adaptation is unavailable,
///   degrade to something workable — for example, detach from the old location's resources
///   and operate without location-specific optimizations — and carry on.
/// * **Not panic** and not block for long.
/// * **Tolerate repetition.** Relocating twice to the same destination, or with `source`
///   equal to `destination`, must be harmless.
/// * **Tolerate omission.** The value must stay correct if relocation is never reported.
/// * **Tolerate a foreign topology.** A value may be handed a `destination` whose
///   [`Topology`] differs from anything it has seen before, for instance when it crosses
///   between two runtimes in the same process. This must remain sound. Hardware-keyed state
///   stays valid across such a move, because [`Core`] and [`MemoryRegion`] describe the
///   same physical machine; runtime-bound state must instead detach, and good performance
///   after such a move is not promised.
///
/// Runtimes that drive relocation carry their own, different obligations. They must call
/// [`relocate`](Self::relocate) only after the value has actually been placed or moved, pass
/// `None` as the source whenever the previous location is unknown rather than inventing one,
/// assign each concurrently live runtime a distinct [`Topology`], and never depend on the
/// callback for correctness — a value they fail to notify must still work.
///
/// # Examples
///
/// A type that rebuilds a *scratch* buffer when the memory region changes. The scratch
/// buffer holds no data between uses, so discarding it is safe; the logical field `name` is
/// untouched.
///
/// ```
/// use thread_aware_core::{Location, MemoryRegion, ThreadAware};
///
/// struct Encoder {
///     name: String,
///     region: Option<MemoryRegion>,
///     /// Reused between calls purely to avoid re-allocating; empty outside a call.
///     scratch: Vec<u8>,
/// }
///
/// impl ThreadAware for Encoder {
///     fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
///         // Only pay for re-allocation when the memory region actually changed.
///         if source.map(Location::memory_region) == Some(destination.memory_region()) {
///             return;
///         }
///
///         // Re-allocate so the scratch space is local to the destination region.
///         self.region = Some(destination.memory_region());
///         self.scratch = Vec::with_capacity(self.scratch.capacity());
///     }
/// }
/// ```
///
/// A composite type forwarding the notification to its fields:
///
/// ```
/// use thread_aware_core::{Location, ThreadAware};
///
/// # struct Encoder;
/// # impl ThreadAware for Encoder {
/// #     fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {}
/// # }
/// struct Session {
///     id: u64,
///     encoder: Encoder,
/// }
///
/// impl ThreadAware for Session {
///     fn relocate(&mut self, source: Option<&Location>, destination: &Location) {
///         self.id.relocate(source, destination);
///         self.encoder.relocate(source, destination);
///     }
/// }
/// ```
pub trait ThreadAware: Send {
    /// Adapts this value in place for the destination location.
    ///
    /// Runtimes call this; implementers of the trait normally do not. It runs after the
    /// value has already been moved, to tell it where it now lives.
    ///
    /// `destination` is where the value runs from now on. `source` is where it ran before,
    /// or `None` when that is unknown — the normal case for initial placement and for
    /// values entering a runtime from outside it. Read `None` as "assume nothing about the
    /// previous location" rather than as an error.
    ///
    /// This method must not fail, must not panic, and must be safe to call repeatedly,
    /// including with `source` equal to `destination`. See the
    /// [trait-level contract](Self#contract) for the full set of expectations.
    ///
    /// Callers should avoid unnecessary relocations, but correctness must never depend on
    /// them doing so.
    fn relocate(&mut self, source: Option<&Location>, destination: &Location);
}
