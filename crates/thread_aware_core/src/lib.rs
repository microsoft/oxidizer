// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]

//! Lets values adapt when a runtime moves them to another CPU core.
//!
//! This crate contains the small API shared by thread-aware libraries:
//!
//! - [`ThreadAware`] tells a value that it has moved.
//! - [`Location`] says where it now runs: which runtime, which core, and which region of
//!   memory sits closest to that core.
//!
//! The crate has no dependencies and is always `no_std`. The companion `thread_aware` crate
//! adds the conveniences: a `#[derive(ThreadAware)]` macro, wrappers for foreign types, and
//! a per-core `Arc`. Depend on this crate directly if you only need to implement the trait.
//!
//! # Why relocation exists
//!
//! Thread-per-core and NUMA-aware runtimes are fast because each worker keeps to itself: it
//! uses memory close to its own core, talks to its own I/O driver, and does not synchronize
//! with other workers. When a value moves to another worker, what used to be close by is now
//! in the wrong place: a cache line shared between cores, memory in a distant region, a
//! handle to another thread's driver.
//!
//! [`ThreadAware`] lets that state fix itself. The runtime moves the value, then calls
//! [`relocate`](ThreadAware::relocate) to say where it now lives.
//!
//! # The two roles
//!
//! **If you write a library or an application**, you implement [`ThreadAware`], usually with
//! the `#[derive(ThreadAware)]` macro. You never call [`relocate`](ThreadAware::relocate)
//! and never build a [`Location`]; the runtime does both, and calls your implementation
//! afterwards. It is a callback, like [`Drop::drop`].
//!
//! **If you write a runtime**, you build a [`Location`] per worker and call
//! [`relocate`](ThreadAware::relocate) after moving a value, passing where it came from and
//! where it now runs.
//!
//! A type made of other types passes the call on to its fields, so one call at the top
//! reaches everything below it. The derive macro and the containers here do that for you.
//!
//! The example below plays the part of the runtime so the order is visible.
//!
//! ```
//! use thread_aware_core::{Core, Location, MemoryRegion, ThreadAware, Topology};
//!
//! // What a library author writes.
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
//! // What the runtime does.
//! let topology = Topology::from(1);
//! let first = Location::new(topology, Core::from(0), MemoryRegion::from(0));
//! let second = Location::new(topology, Core::from(3), MemoryRegion::from(1));
//!
//! let mut worker = Worker { core: None };
//!
//! worker.relocate(None, &first); // first placement, no previous location
//! worker.relocate(Some(&first), &second); // moved to another core
//!
//! assert_eq!(worker.core, Some(Core::from(3)));
//! ```
//!
//! # Performance, not correctness
//!
//! Relocation is an optimization, not a guarantee. Your value has to stay correct if the
//! call never comes, comes twice, or reports the same source and destination. Missing calls
//! are normal: a value can reach another thread through `std::thread::spawn`, a channel, or
//! a runtime that knows nothing about this trait. That may make things slower, but it must
//! never cause a panic, a deadlock, or a wrong answer.
//!
//! It is not a hot path either. Expect about one relocation per object graph per job or
//! request, after which the value is simply used. Prefer avoiding synchronization over
//! saving a few cycles.
//!
//! # What the ids mean
//!
//! [`Core`] and [`MemoryRegion`] name hardware, not slots in a worker list, so two runtimes
//! on the same machine both report core 2 as the same [`Core`] and can share state keyed on
//! it. That only holds while they derive the ids the same way, for example from the
//! numbering the operating system reports. Nothing checks it, because [`Core::from`] accepts
//! any `u16`, and if two runtimes number hardware differently then state shared between them
//! is wrong, not just slow. Share across runtimes only when you control every one of them.
//!
//! [`Topology`] says which runtime produced the location. It does not change what the other
//! two mean; it tells you whether you are still inside the runtime that gave you your
//! resources. So use only the ids your state depends on:
//!
//! - State tied to hardware, such as a per-core cache, can use [`Core`] or [`MemoryRegion`]
//!   alone, and survives a move between runtimes as long as what backs it is not owned by
//!   one of them.
//! - State tied to a runtime, such as a scheduler, a driver handle, or memory it allocated,
//!   has to check [`Topology`] too and let go when it changes. When in doubt, assume this.
//!
//! The ids mean nothing beyond identity. They need not start at zero or run consecutively,
//! there is no count, and you cannot list them. Keep per-location state in a map keyed by
//! the id rather than an array you index into.
//!
//! # Relation to `Send`
//!
//! [`ThreadAware`] requires [`Send`], and in that order: a value is sent to another thread
//! first, then told where it landed. [`Send`] is what makes the move safe, and
//! [`ThreadAware`] adds nothing to it.
//!
//! # Threads and cores
//!
//! This crate assumes one worker per core, so "moved to another thread" and "moved to
//! another core" mean the same thing. A [`Location`] cannot describe two workers on one
//! core: a runtime that puts several threads on a core, or leaves them unpinned, has to give
//! each worker its own [`Core`] id, and those workers then no longer share per-core state.
//! There is no way to say "not pinned".
//!
//! # Provided implementations
//!
//! Types with nothing tied to a location get an empty implementation: primitives, the
//! location ids, `Duration`, strings, safe function pointers, and, with the `std` feature,
//! paths.
//!
//! Containers pass the call through to what they hold: [`Option`], [`Result`], arrays,
//! slices, `Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples of up to twelve elements, and map
//! values. A `Cow` only forwards when it owns its data.
//!
//! Map keys are left alone, since changing one could change its hash or ordering and break
//! the map. Sets are not implemented at all for the same reason, so a `HashSet` or
//! `BTreeSet` field is simply not [`ThreadAware`].
//!
//! `Arc` is left out too: whether a shared allocation should stay shared across cores or be
//! split per core depends on what is inside it. Use the per-core `Arc` in `thread_aware`
//! when splitting is the right answer.
//!
//! # Crate features
//!
//! * The **`std` Cargo feature** *(off by default)* adds implementations for standard
//!   library types such as `HashMap`, `Path` and `PathBuf`. Without it the crate needs only
//!   `alloc`.

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod impls;
mod location;

#[doc(inline)]
pub use location::{Core, Location, MemoryRegion, Topology};

/// Tells a value that it has moved to a different [`Location`].
///
/// Implement this when part of your type depends on where it runs: memory in a particular
/// region, a handle to a thread-local driver, a shard index, a cached core id.
/// [`relocate`](Self::relocate) is where you bring that back in line. You do not call it
/// yourself; see [the two roles](crate#the-two-roles).
///
/// Implement or derive it if your type might end up inside something a runtime relocates,
/// including simple types that do nothing on relocation, since an empty implementation is
/// what lets an enclosing type derive the trait. Do not implement it just because a type is
/// [`Send`], and never put anything correctness depends on inside
/// [`relocate`](Self::relocate). It may only affect performance.
///
/// # Usage patterns
///
/// Implementations usually look like one of these:
///
/// 1. **Do nothing.** Nothing in the type depends on where it runs. An empty body is a
///    complete implementation, and it is what primitives do.
///
///    ```
///    use thread_aware_core::{Location, ThreadAware};
///
///    struct RequestId(u64);
///
///    impl ThreadAware for RequestId {
///        fn relocate(&mut self, _source: Option<&Location>, _destination: &Location) {}
///    }
///    ```
/// 2. **Remember where you are.** Store the new [`Core`] or [`MemoryRegion`] and use it
///    later.
/// 3. **Swap a resource.** Check the id it depends on: [`MemoryRegion`] for a pool,
///    [`Topology`] for a driver handle. If it changed, let the old one go and get one for
///    the new location, moving out any real data it holds first.
/// 4. **Pass it on.** A type made of other types calls `relocate` on each field. Use
///    `#[derive(ThreadAware)]` rather than writing this by hand.
///
/// **Detaching** means releasing a resource and leaving the field empty, so the value can
/// pick up a new one later or run without it. Such a field needs a type that can be empty,
/// like `Option<Handle>`.
///
/// # Rules
///
/// [`relocate`](Self::relocate) cannot fail and has no way to report an error, so
/// implementations must:
///
/// * **Keep real data.** You may rebuild anything that exists only for speed: caches, pools,
///   scratch buffers, handles. You may not lose or change anything a user of the value can
///   see. A cache still holding writes that have not been flushed is real data, so move it
///   rather than dropping it.
/// * **Stay correct.** If you cannot do the ideal thing, be slower instead; keeping the old
///   resource is fine. What you hold must keep working even if this is never called, so a
///   driver handle has to stay usable from the new core. Relocation makes it closer, not
///   valid.
/// * **Do not panic or block.** This runs while the runtime is placing work, so no network
///   or disk I/O, no waiting on another worker, no contended lock. If something would block,
///   let go now and pick it up again on first use.
/// * **Handle repeated calls.** Relocating to the same place, or with `source` equal to
///   `destination`, has to be harmless, and should be cheap: compare the ids you care about
///   and return early when nothing changed.
/// * **Handle no call at all.** The value has to stay correct either way.
/// * **Handle a move between runtimes.** A `destination` may come from a different runtime,
///   and that must stay sound. State tied to a runtime should let go; state tied to hardware
///   may still be fine, subject to the caveat in
///   [what the ids mean](crate#what-the-ids-mean). Things may be slower afterwards, but not
///   forever: back on a runtime it can serve, the value should pick up what it released.
///
/// You can clone a [`Location`] and keep it, but its ids only mean something while the
/// runtime that made them is still running.
///
/// Runtimes have their own rules. Call [`relocate`](Self::relocate) only after really moving
/// the value, pass `None` when the old location is unknown, give each running runtime its
/// own [`Topology`], and never rely on the call for correctness. Nothing checks any of this.
///
/// # Examples
///
/// A type that rebuilds a scratch buffer when the memory region changes. The buffer holds
/// nothing between calls, so throwing it away is safe. The `name` field is real data and is
/// left alone.
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
/// A type that passes the call on to its fields:
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
    /// Updates this value for the location it has moved to.
    ///
    /// You write this method, but you do not normally call it. The runtime does, after
    /// moving the value.
    ///
    /// `destination` is where the value runs from now on. `source` is where it ran before,
    /// or `None` when that is unknown, which is normal for a first placement or a value
    /// arriving from outside the runtime. `None` means "assume nothing", not "error".
    ///
    /// This must not fail, must not panic, and must be safe to call more than once,
    /// including when `source` equals `destination`. See the [rules](Self#rules) for the
    /// rest.
    fn relocate(&mut self, source: Option<&Location>, destination: &Location);
}
