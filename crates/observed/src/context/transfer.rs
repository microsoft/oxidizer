// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Captured enrichment context that can cross a thread or task boundary.

use std::any::type_name;

use crate::SinkId;
use crate::enrichment::{Enrichment, EnrichmentTransfer};

/// Captured sink context (enrichment) for cross-thread transfer.
///
/// Created by [`Sink::transfer_context`](crate::Sink::transfer_context). Restore it on
/// another thread or task by wrapping a future with
/// [`EnrichFutureExt::attach`](crate::enrichment::EnrichFutureExt::attach) (recommended
/// for async code), or synchronously via [`Transfer::apply_current_thread`].
///
/// This is the integration surface for spawners, `tower` layers and similar
/// middleware; code that spawns through `observed_rt` or `oxidizer_rt` gets
/// enrichment propagation without touching it.
///
/// An integration that wants to contribute entries of its own should add them
/// here, with [`with_enrichment`](Self::with_enrichment) or
/// [`with_enrichment_for`](Self::with_enrichment_for), rather than wrapping the
/// attached future in another `enrich` layer: entries carried inside the
/// transfer are independent of wrapper order, so they survive the future being
/// boxed or attached again further out.
///
/// Applying a transfer mutates the current thread's enrichment for the guard's
/// lifetime, so emissions made through the *original* sink on that thread also see
/// the transferred entries.
///
/// See the [Enrichment - cross-thread transfer](crate#transferring-enrichment-across-threads-and-tasks)
/// section for the full workflow.
#[derive(Clone, thread_aware::ThreadAware)]
#[must_use]
pub struct Transfer {
    enrichment: EnrichmentTransfer,
}

impl std::fmt::Debug for Transfer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

impl Transfer {
    pub(crate) fn new(enrichment: EnrichmentTransfer) -> Self {
        Self { enrichment }
    }

    /// Adds extra enrichment that is applied along with the captured context.
    ///
    /// The additional enrichment layers on top of the entries already
    /// captured by [`Transfer`], so it is visible on every thread the
    /// transfer is applied to.
    ///
    /// The entries are **global**: every non-isolated sink that observes this
    /// transfer sees them. Use
    /// [`with_enrichment_for`](Self::with_enrichment_for) to restrict them to a
    /// single sink.
    pub fn with_enrichment(mut self, additional_enrichment: impl Enrichment) -> Self {
        self.enrichment.push(additional_enrichment);
        self
    }

    /// Adds extra enrichment that only `target` observes.
    ///
    /// Applied along with the captured context. The targeted counterpart of
    /// [`with_enrichment`](Self::with_enrichment),
    /// carrying the same targeting semantics as
    /// [`EnrichFutureExt::enrich_for`](crate::enrichment::EnrichFutureExt::enrich_for):
    /// other sinks skip the entries entirely.
    ///
    /// Because the entries travel inside the transfer rather than in a wrapper
    /// around the future, this is independent of wrapper order. It is therefore
    /// the way to attach targeted enrichment in the shapes where
    /// [`Transferred::enrich_for`](crate::context::Transferred::enrich_for) is
    /// not selected - nested transfers, a boxed or type-erased future, or
    /// generic code bounded on `EnrichFutureExt`.
    pub fn with_enrichment_for(mut self, target: SinkId, additional_enrichment: impl Enrichment) -> Self {
        self.enrichment.push_for(target, additional_enrichment);
        self
    }

    /// Applies the captured enrichment to the **current thread** for the
    /// lifetime of the returned guard.
    ///
    /// The returned guard keeps the enrichment active for its lifetime and
    /// removes it again when dropped. Takes `&self` so the same transfer can
    /// be applied repeatedly (e.g. once per poll of an attached future).
    ///
    /// # Warning
    ///
    /// Because this mutates a thread-local, the guard **must not be held across
    /// an `.await`**: while the task is suspended the enrichment would remain
    /// active and leak into unrelated tasks the runtime schedules on the same
    /// thread. For async code use
    /// [`EnrichFutureExt::attach`](crate::enrichment::EnrichFutureExt::attach),
    /// which re-applies the transfer on each poll and drops the guard before the
    /// future yields.
    #[must_use]
    pub fn apply_current_thread(&self) -> impl Drop {
        self.enrichment.apply()
    }
}
