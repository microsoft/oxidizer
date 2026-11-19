// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ThreadAware`] trait.

use crate::Thread;

/// A type that adapts after it arrives on a different [`Thread`].
///
/// Implement this trait when part of a type depends on where it runs. Examples include nearby
/// memory, a thread-local driver handle, a shard index, or a cached thread id.
/// [`relocate`](Self::relocate) adapts that state. A runtime calls it as described in
/// [driving relocation](crate#runtime-authors-driving-relocation).
///
/// Implement or derive the trait for each field that relocation traverses. Use an explicit skip
/// policy or wrapper for other fields. An empty implementation lets an enclosing type derive the
/// trait. Do not implement it only because a type is [`Send`]. [`relocate`](Self::relocate) can
/// affect performance, but correctness must not depend on it.
///
/// # Implementing
///
/// Implementations usually use one of these patterns:
///
/// 1. Do nothing if no state depends on the thread.
///
///    ```
///    use thread_aware_core::{Thread, ThreadAware};
///
///    struct RequestId(u64);
///
///    impl ThreadAware for RequestId {
///        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
///    }
///    ```
/// 2. Record the destination thread id, [`NumaNode`](crate::NumaNode), or
///    [`Owner`](crate::Owner).
/// 3. Adapt an optimization when its relevant id changes.
/// 4. Forward `relocate` to each field. Prefer `#[derive(ThreadAware)]` over a manual
///    implementation.
///
/// To detach a resource means to release it and leave the field empty. The value can get a new
/// resource later or continue without one. The field needs an empty state such as
/// `Option<Handle>`.
///
/// # Requirements
///
/// [`relocate`](Self::relocate) cannot return an error. Every implementation must meet these
/// requirements:
///
/// - Preserve data. An implementation can rebuild caches, pools, scratch buffers, and handles
///   that exist only for performance. It must preserve observable data. For example, it must not
///   drop a cache that contains writes that are not flushed.
///
/// - Remain correct. If adaptation is not available, keep usable old state and accept lower
///   performance. The value must also work if the runtime never calls this method.
///
/// - Do not panic or perform long blocking work. Do not take a contended lock, perform network or
///   disk I/O, or wait for external progress. Brief in-memory coordination is allowed. Defer longer
///   work and keep usable state.
///
/// - Accept repeated calls. A call with equal relevant coordinates must be safe and cheap. Compare
///   the relevant ids and return early if they did not change.
///
/// - Remain correct without a call.
///
/// - Accept an unfamiliar [`Thread`]. The destination can use a new OS thread or an
///   [`Owner`](crate::Owner) from another runtime. The value can keep usable state, remove an
///   optimization, or prepare replacement state. See [what the ids mean](crate#what-the-ids-mean)
///   before sharing state by [`NumaNode`](crate::NumaNode).
///
/// A [`Thread`] can be cloned and stored. Its thread id remains unique after the OS thread exits.
/// The stored coordinate does not keep the thread or runtime operational.
///
/// A runtime calls [`relocate`](Self::relocate) on the thread that owns the moved value. It passes
/// `None` if the previous [`Thread`] is unknown. It creates one [`Owner`](crate::Owner) for each
/// runtime. It must not depend on relocation for correctness. Nothing enforces these requirements.
///
/// # Examples
///
/// This type releases a scratch buffer when the nearest memory changes. The next use can allocate
/// the buffer near the new node. The buffer holds no data between calls. The `name` field remains
/// unchanged because it is data.
///
/// ```
/// use thread_aware_core::{NumaNode, Thread, ThreadAware};
///
/// struct Encoder {
///     name: String,
///     numa_node: Option<NumaNode>,
///     /// Reused between calls to avoid allocation. It is empty outside a call.
///     scratch: Vec<u8>,
/// }
///
/// impl ThreadAware for Encoder {
///     fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
///         // Adapt only if the nearest memory changed.
///         if source.map(Thread::numa_node) == Some(destination.numa_node()) {
///             return;
///         }
///
///         // Record the node and drop the old buffer. `Vec` uses the application's allocator.
///         // The next allocation can use the destination thread's locality if the allocator
///         // supports it.
///         self.numa_node = Some(destination.numa_node().clone());
///         self.scratch = Vec::new();
///     }
/// }
/// ```
///
/// A type that forwards the call to its fields:
///
/// ```
/// use thread_aware_core::{Thread, ThreadAware};
///
/// # struct Encoder;
/// # impl ThreadAware for Encoder {
/// #     fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
/// # }
/// struct Session {
///     id: u64,
///     encoder: Encoder,
/// }
///
/// impl ThreadAware for Session {
///     fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
///         self.id.relocate(source, destination);
///         self.encoder.relocate(source, destination);
///     }
/// }
/// ```
pub trait ThreadAware: Send {
    /// Adapts this value to its current [`Thread`].
    ///
    /// A runtime calls this method after it moves the value.
    ///
    /// `destination` describes the thread that runs this method and owns the value. It is not a
    /// request to move the value. `source` describes the previous thread. It is `None` for an
    /// unknown source, a first placement, or a value from outside the runtime. `None` is not an
    /// error.
    ///
    /// This method cannot return an error. It must not panic. Repeated calls and equal coordinates
    /// must be safe. See the [requirements](Self#requirements) and [examples](Self#examples).
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread);
}
