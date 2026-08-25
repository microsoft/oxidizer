// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![no_std]

//! Lets values adapt when a runtime moves them to another thread.
//!
//! This crate contains the small API shared by thread-aware libraries:
//!
//! - [`ThreadAware`] tells a value that it has moved.
//! - [`Place`] says where it now runs: which runtime, which thread, and which memory is
//!   closest to it.
//!
//! The crate has no dependencies. It also works without `std`: turn off default features,
//! and [`Place`] loses its thread id, keeping [`Origin`] and [`Numa`]. The companion
//! `thread_aware` crate adds the conveniences: a `#[derive(ThreadAware)]` macro, wrappers
//! for foreign types, and a per-core `Arc`. Depend on this crate directly if you only need
//! to implement the trait.
//!
//! # Why relocation exists
//!
//! Thread-per-core and NUMA-aware runtimes are fast because each worker keeps to itself: it
//! uses memory close to its own thread, talks to its own I/O driver, and does not
//! synchronize with other workers. When a value moves to another worker, what used to be
//! close by is now in the wrong place: a cache line shared between threads, memory in a
//! distant region, a handle to another thread's driver.
//!
//! [`ThreadAware`] lets that state fix itself. The runtime moves the value, then calls
//! [`relocate`](ThreadAware::relocate) to say where it now lives.
//!
//! # The two roles
//!
//! **If you write a library or an application**, you implement [`ThreadAware`], usually with
//! the `#[derive(ThreadAware)]` macro. You never call [`relocate`](ThreadAware::relocate)
//! and never build a [`Place`]; the runtime does both, and calls your implementation
//! afterwards. It is a callback, like [`Drop::drop`].
//!
//! **If you write a runtime**, you build a [`Place`] per worker and call
//! [`relocate`](ThreadAware::relocate) after moving a value, passing where it came from and
//! where it now runs.
//!
//! A type made of other types passes the call on to its fields, so one call at the top
//! reaches everything below it. The derive macro and the containers here do that for you.
//!
//! The example below plays the part of the runtime so the order is visible.
//!
//! ```
//! use std::thread;
//!
//! use thread_aware_core::{Numa, Origin, Place, ThreadAware};
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
//! let first = Place::new(origin, here, Numa::from(0));
//! let second = Place::new(origin, there, Numa::from(1));
//!
//! let mut worker = Worker { thread: None };
//!
//! worker.relocate(None, &first); // first placement, no previous place
//! worker.relocate(Some(&first), &second); // moved to another thread
//!
//! assert_eq!(worker.thread, Some(there));
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
//! The thread id is `std::thread::ThreadId`. It identifies one thread and nothing else, and
//! is unique among the threads alive at the same time, so state keyed on it is never shared
//! by accident, not even between two runtimes in the same process.
//!
//! [`Numa`] identifies the memory closest to that thread. Unlike the thread id it is shared:
//! every thread near the same memory reports the same [`Numa`]. That is what makes it useful
//! for state you want to share within a region but not across the machine.
//!
//! That sharing only works while every runtime in the process numbers the regions the same
//! way, for example from the numbering the operating system reports. Nothing checks it, and
//! if two runtimes number them differently then state shared between them is wrong, not just
//! slow. Share across runtimes only when you control every one of them.
//!
//! [`Origin`] identifies the runtime that produced the place. Thread ids already tell
//! threads apart, so this is about ownership: it lets a value notice that it has crossed
//! into a different runtime and release anything the old one owned.
//!
//! So use only the ids your state depends on:
//!
//! - State that must not be shared at all, such as a per-thread cache or a handle to a
//!   thread-local driver, keys on the thread id and is replaced whenever the thread changes.
//! - State that only cares about memory locality, such as a buffer pool, keys on [`Numa`]
//!   and survives a move to another thread near the same memory.
//! - State owned by the runtime, such as a scheduler handle, also checks [`Origin`] and lets
//!   go when it changes.
//!
//! The ids mean nothing beyond identity. [`Origin`] and [`Numa`] need not start at zero or
//! run consecutively, there is no count, and you cannot list the places in use. Keep
//! per-place state in a map keyed by the id rather than an array you index into.
//!
//! Without `std` there is no thread id: `Place::new` and `Place::thread` are gone and only
//! [`Origin`] and [`Numa`] remain. A `no_std` library can still implement [`ThreadAware`]
//! and use whatever it is handed; the runtime that drives relocation needs `std` anyway.
//!
//! # Relation to `Send`
//!
//! [`ThreadAware`] requires [`Send`], and in that order: a value is sent to another thread
//! first, then told where it landed. [`Send`] is what makes the move safe, and
//! [`ThreadAware`] adds nothing to it.
//!
//! # Provided implementations
//!
//! Types with nothing tied to a place get an empty implementation: primitives and their
//! non-zero variants, the place ids, `Duration`, strings, safe function pointers of up to
//! twelve parameters, and, with the `std` feature, paths.
//!
//! Containers pass the call through to what they hold: [`Option`], [`Result`], arrays,
//! slices, `Vec`, `VecDeque`, `Box`, `Cow`, cells, tuples of up to twelve elements, and map
//! values. A `Cow` only forwards when it owns its data.
//!
//! Map keys are left alone, since changing one could change its hash or ordering and break
//! the map. Sets are not implemented at all for the same reason, so a `HashSet` or
//! `BTreeSet` field is simply not [`ThreadAware`].
//!
//! `Arc` is left out too: whether a shared allocation should stay shared across threads or
//! be split per thread depends on what is inside it. Use the per-core `Arc` in
//! `thread_aware` when splitting is the right answer.
//!
//! # Crate features
//!
//! * The **`std` Cargo feature** *(enabled by default)* provides the thread id half of
//!   [`Place`] and implementations for standard library types such as `HashMap`, `Path` and
//!   `PathBuf`. Turn it off for `no_std`; the crate then needs only `alloc`.

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod impls;
mod place;

#[doc(inline)]
pub use place::{Numa, Origin, Place};

/// Tells a value that it has moved to a different [`Place`].
///
/// Implement this when part of your type depends on where it runs: memory near a particular
/// node, a handle to a thread-local driver, a shard index, a cached thread id.
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
///    use thread_aware_core::{Place, ThreadAware};
///
///    struct RequestId(u64);
///
///    impl ThreadAware for RequestId {
///        fn relocate(&mut self, _source: Option<&Place>, _destination: &Place) {}
///    }
///    ```
/// 2. **Remember where you are.** Store the new thread id, [`Numa`] or [`Origin`] and use
///    it later.
/// 3. **Swap a resource.** Check the id it depends on: [`Numa`] for a buffer pool, the
///    thread id for a driver handle, [`Origin`] for anything the runtime owns. If it
///    changed, let the old one go and get one for the new place, moving out any real data it
///    holds first.
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
///   driver handle has to stay usable from the new thread. Relocation makes it closer, not
///   valid.
/// * **Do not panic or block.** This runs while the runtime is placing work, so no network
///   or disk I/O, no waiting on another worker, no contended lock. If something would block,
///   let go now and pick it up again on first use.
/// * **Handle repeated calls.** Relocating to the same place, or with `source` equal to
///   `destination`, has to be harmless, and should be cheap: compare the ids you care about
///   and return early when nothing changed.
/// * **Handle no call at all.** The value has to stay correct either way.
/// * **Handle a place you know nothing about.** A `destination` may name a thread the value
///   has never seen, and carry an [`Origin`] belonging to another runtime. That must stay
///   sound. Let go of anything the old runtime owned; state keyed on [`Numa`] may still be
///   fine, subject to the caveat in [what the ids mean](crate#what-the-ids-mean). Things may
///   be slower afterwards, but not forever: back on a place it can serve, the value should
///   pick up what it released.
///
/// You can clone a [`Place`] and keep it, but a thread id only means something while that
/// thread is alive, and an [`Origin`] only while that runtime is.
///
/// Runtimes have their own rules. Call [`relocate`](Self::relocate) only after really moving
/// the value, pass `None` when the old place is unknown, give each running runtime its own
/// [`Origin`], and never rely on the call for correctness. Nothing checks any of this.
///
/// # Examples
///
/// A type that rebuilds a scratch buffer when the nearest memory changes. The buffer holds
/// nothing between calls, so throwing it away is safe. The `name` field is real data and is
/// left alone.
///
/// ```
/// use thread_aware_core::{Numa, Place, ThreadAware};
///
/// struct Encoder {
///     name: String,
///     numa: Option<Numa>,
///     /// Reused between calls purely to avoid re-allocating; empty outside a call.
///     scratch: Vec<u8>,
/// }
///
/// impl ThreadAware for Encoder {
///     fn relocate(&mut self, source: Option<&Place>, destination: &Place) {
///         // Only pay for re-allocation when the nearest memory actually changed.
///         if source.map(Place::numa) == Some(destination.numa()) {
///             return;
///         }
///
///         // Re-allocate so the scratch space is local to the destination.
///         self.numa = Some(destination.numa());
///         self.scratch = Vec::with_capacity(self.scratch.capacity());
///     }
/// }
/// ```
///
/// A type that passes the call on to its fields:
///
/// ```
/// use thread_aware_core::{Place, ThreadAware};
///
/// # struct Encoder;
/// # impl ThreadAware for Encoder {
/// #     fn relocate(&mut self, _source: Option<&Place>, _destination: &Place) {}
/// # }
/// struct Session {
///     id: u64,
///     encoder: Encoder,
/// }
///
/// impl ThreadAware for Session {
///     fn relocate(&mut self, source: Option<&Place>, destination: &Place) {
///         self.id.relocate(source, destination);
///         self.encoder.relocate(source, destination);
///     }
/// }
/// ```
pub trait ThreadAware: Send {
    /// Updates this value for the place it has moved to.
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
    fn relocate(&mut self, source: Option<&Place>, destination: &Place);
}
