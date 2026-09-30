// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment propagation for spawned tasks.
//!
//! Enrichment propagation captures the current enrichment stack at spawn time and restores it each
//! time a spawned task is polled. This associates telemetry emitted from a child task with the
//! parent task that spawned it.
//!
//! See the *Enrichment* section in the `observed` crate docs for the full
//! model (storage, scoping, cross-thread transfer).

use std::any::type_name;

/// Enrichment stack captured from a parent task, ready to be applied to a child task
/// when it is polled.
///
/// See [`observed::context::Transfer`] for details on context propagation.
#[derive(Clone)]
pub(crate) struct CapturedContext(observed::context::Transfer);

impl std::fmt::Debug for CapturedContext {
    #[cfg_attr(coverage_nightly, coverage(off))] // Never render captured enrichment data.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

impl CapturedContext {
    /// Captures the current enrichment stack from `sink`.
    ///
    /// Call this from the parent task before spawning a child task so the child inherits the
    /// parent's enrichment stack when it is polled.
    ///
    /// See [`observed::Sink::transfer_context`].
    pub(crate) fn capture(sink: &observed::Sink) -> Self {
        Self(sink.transfer_context())
    }

    /// Replaces the current enrichment stack with the one captured in this object.
    ///
    /// Called from the child task each time it is polled. The enrichment stays active for the
    /// lifetime of the returned guard, which must therefore be held for as long as the task is
    /// being polled. The guard is deliberately opaque and carries no `Drop` bound: it restores
    /// the previous stack through the `Drop` impl of the inner `observed` guard it wraps.
    ///
    /// See [`observed::context::Transfer::apply_current_thread`].
    #[must_use = "the enrichment is removed as soon as the returned guard is dropped"]
    pub(crate) fn apply(&self) -> impl Sized {
        self.0.apply_current_thread()
    }
}
