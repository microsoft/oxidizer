// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ThreadAware`] trait.

use crate::Thread;

/// A type that adapts when it is moved to a different [`Thread`].
///
/// Implement this trait when part of a type depends on where it runs: memory near a
/// particular node, a handle to a thread-local driver, a shard index, a cached thread id.
/// [`relocate`](Self::relocate) brings that state back into line. Implementors do not call
/// it themselves; a runtime does, as described in
/// [driving relocation](crate#runtime-authors-driving-relocation).
///
/// Implement or derive it for any type that may end up inside a value a runtime relocates,
/// including types that do nothing on relocation, since an empty implementation is what lets
/// an enclosing type derive the trait. Do not implement it merely because a type is
/// [`Send`], and never put anything correctness depends on inside
/// [`relocate`](Self::relocate); it may affect performance only.
///
/// # Implementing
///
/// Implementations usually take the following shapes.
///
/// 1. **Do nothing.** Nothing in the type depends on where it runs. An empty body is a
///    complete implementation, and is what the primitive types do.
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
/// 2. **Record the destination.** Store the new thread id, [`NumaNode`](crate::NumaNode) or
///    [`Owner`](crate::Owner) for later use.
/// 3. **Adapt an optimization.** Compare the id it depends on: [`NumaNode`](crate::NumaNode)
///    for a buffer pool, the thread id for a driver handle, [`Owner`](crate::Owner) for
///    runtime-specific state. If it changed, an implementation may replace, detach, or retain
///    the old resource according to what remains usable and worthwhile.
/// 4. **Forward to fields.** A type composed of other types calls `relocate` on each field.
///    Prefer `#[derive(ThreadAware)]` to writing this by hand.
///
/// To *detach* is to release a resource and leave the field empty, so that the value can
/// acquire a new one later or run without it. Such a field needs a type that can be empty,
/// such as `Option<Handle>`.
///
/// # Requirements
///
/// [`relocate`](Self::relocate) cannot fail and has no way to report an error, so every
/// implementation must:
///
/// * **Preserve real data.** Anything that exists only for speed may be rebuilt: caches,
///   pools, scratch buffers, handles. Nothing observable through the value may be lost or
///   altered. A cache still holding writes that have not been flushed is real data, and must
///   be moved rather than dropped.
///
/// * **Remain correct.** When the ideal adaptation is unavailable, being slower is preferred
///   to being wrong, and keeping the old resource is acceptable. Whatever the value holds
///   must keep working even if this method is never called, so a driver handle must remain
///   usable from the new thread. Relocation brings it closer; it does not make it valid.
///
/// * **Neither panic nor perform long blocking work.** This runs while the runtime is placing work,
///   so it performs no network or disk I/O and does not wait for external progress. Brief in-memory
///   coordination is acceptable, but caller code must not run while a shared map or collection lock
///   is held. Defer longer adaptation; retaining usable state is acceptable.
///
/// * **Tolerate repeated calls.** Relocating to the same [`Thread`], or with `source` equal to
///   `destination`, is harmless, and should also be cheap: compare the relevant ids and
///   return early when nothing has changed.
///
/// * **Tolerate no call at all.** The value remains correct either way.
///
/// * **Tolerate an unfamiliar [`Thread`].** A `destination` may name an OS thread the value
///   has never seen and carry an [`Owner`](crate::Owner) belonging to another runtime. The
///   value must remain sound whether it retains usable state, abandons an optimization, or
///   prepares to replace runtime-specific state later. State keyed on
///   [`NumaNode`](crate::NumaNode) may remain useful, subject to the caveat in
///   [what the ids mean](crate#what-the-ids-mean).
///
/// A [`Thread`] may be cloned and retained, but the thread id it holds is meaningful only
/// while that thread is alive, and an [`Owner`](crate::Owner) only while that runtime is.
///
/// Runtimes carry their own requirements. They call [`relocate`](Self::relocate) only after
/// the value has actually moved, pass `None` when no previous [`Thread`] is known, build one
/// [`Owner`](crate::Owner) per runtime, and never rely on the call for correctness. Nothing
/// enforces any of this.
///
/// # Examples
///
/// A type that releases a scratch buffer when the nearest memory changes, so that the next
/// use allocates near the new node. The buffer holds nothing between calls, so discarding it
/// is safe, while the `name` field is real data and is left alone.
///
/// ```
/// use thread_aware_core::{NumaNode, Thread, ThreadAware};
///
/// struct Encoder {
///     name: String,
///     numa_node: Option<NumaNode>,
///     /// Reused between calls purely to avoid re-allocating; empty outside a call.
///     scratch: Vec<u8>,
/// }
///
/// impl ThreadAware for Encoder {
///     fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
///         // Only pay for this when the nearest memory actually changed.
///         if source.map(Thread::numa_node) == Some(destination.numa_node()) {
///             return;
///         }
///
///         // Record the node and drop the buffer allocated near the old one. `Vec` cannot
///         // choose a node itself, so placement comes from the allocator the application
///         // installs; releasing here is what gives it the chance to allocate near
///         // `numa_node` on the next use.
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
    /// Adapts this value to the [`Thread`] it now occupies.
    ///
    /// Implementors provide this method but do not normally call it. A runtime calls it
    /// after moving the value.
    ///
    /// `destination` is where the value runs from now on. `source` is where it ran before,
    /// or `None` when that is unknown, which is normal for a first placement or for a value
    /// arriving from outside the runtime. `None` means the implementation can assume nothing
    /// about the previous [`Thread`]; it does not indicate an error.
    ///
    /// This method cannot fail, must not panic, and is safe to call more than once,
    /// including with `source` equal to `destination`. See the
    /// [requirements](Self#requirements) for the rest, and the trait-level
    /// [examples](Self#examples) for implementations.
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread);
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_obj_safe;

    use super::ThreadAware;

    // `dyn ThreadAware` is part of the stable surface, so anything added to the trait later
    // has to keep it dyn-compatible.
    assert_obj_safe!(ThreadAware);
}
