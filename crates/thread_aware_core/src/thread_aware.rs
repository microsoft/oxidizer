// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`ThreadAware`] trait.

use crate::Place;

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
/// 2. **Remember where you are.** Store the new thread id,
///    [`NumaNode`](crate::NumaNode) or [`Origin`](crate::Origin) and use it later.
/// 3. **Swap a resource.** Check the id it depends on: [`NumaNode`](crate::NumaNode) for a
///    buffer pool, the thread id for a driver handle, [`Origin`](crate::Origin) for anything
///    the runtime owns. If it changed, let the old one go and get one for the new place,
///    moving out any real data it holds first.
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
///   has never seen, and carry an [`Origin`](crate::Origin) belonging to another runtime.
///   That must stay sound. Let go of anything the old runtime owned; state keyed on
///   [`NumaNode`](crate::NumaNode) may still be fine, subject to the caveat in
///   [what the ids mean](crate#what-the-ids-mean). Things may be slower afterwards, but not
///   forever: back on a place it can serve, the value should pick up what it released.
///
/// You can clone a [`Place`] and keep it, but a thread id only means something while that
/// thread is alive, and an [`Origin`](crate::Origin) only while that runtime is.
///
/// Runtimes have their own rules. Call [`relocate`](Self::relocate) only after really moving
/// the value, pass `None` when the old place is unknown, give each running runtime its own
/// [`Origin`](crate::Origin), and never rely on the call for correctness. Nothing checks any
/// of this.
///
/// # Examples
///
/// A type that rebuilds a scratch buffer when the nearest memory changes. The buffer holds
/// nothing between calls, so throwing it away is safe. The `name` field is real data and is
/// left alone.
///
/// ```
/// use thread_aware_core::{NumaNode, Place, ThreadAware};
///
/// struct Encoder {
///     name: String,
///     numa_node: Option<NumaNode>,
///     /// Reused between calls purely to avoid re-allocating; empty outside a call.
///     scratch: Vec<u8>,
/// }
///
/// impl ThreadAware for Encoder {
///     fn relocate(&mut self, source: Option<&Place>, destination: &Place) {
///         // Only pay for re-allocation when the nearest memory actually changed.
///         if source.map(Place::numa_node) == Some(destination.numa_node()) {
///             return;
///         }
///
///         // Re-allocate so the scratch space is local to the destination.
///         self.numa_node = Some(destination.numa_node());
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
