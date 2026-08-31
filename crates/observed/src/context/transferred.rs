// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Future wrapper that restores a captured [`Transfer`] on every poll.

use core::task;
use std::any::type_name;
use std::pin::Pin;
use std::task::Poll;

use crate::context::Transfer;
use crate::enrichment::{EnrichFutureExt, Enriched, Enrichment};
use crate::{Sink, SinkId};

/// A future wrapper that restores a captured [`Transfer`]
/// on every poll.
///
/// Created by [`EnrichFutureExt::attach`]. This is integration plumbing -
/// spawners, `tower` layers and similar middleware - rather than something most
/// code constructs directly; `observed_rt` and `oxidizer_rt` already propagate
/// enrichment for tasks spawned through them.
///
/// See the [Enrichment - cross-thread transfer](crate#transferring-enrichment-across-threads-and-tasks)
/// section for details.
#[pin_project::pin_project]
#[must_use]
pub struct Transferred<T> {
    #[pin]
    inner: T,
    context_transfer: Transfer,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Transferred<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<T> Transferred<T> {
    pub(crate) fn new(inner: T, context_transfer: Transfer) -> Self {
        Self { inner, context_transfer }
    }
}

impl<T: Future> Transferred<T> {
    /// Wraps this future so that `enrichment` is active on every poll.
    ///
    /// The entries are layered on top of the transferred context.
    ///
    /// Applying a transfer replaces the captured slot's chain, so the two
    /// wrappers are not commutative on their own. This inherent method exists so
    /// that they behave as if they were in the common case: it re-orders the
    /// wrappers, restoring the transfer first and pushing `enrichment` on top of
    /// it. For a single `attach` on a plain, non-boxed future, both
    /// `future.enrich(&sink, e).attach(transfer)` and
    /// `future.attach(transfer).enrich(&sink, e)` therefore leave `e` visible on
    /// every event the future emits. That case is common but limited - see
    /// [Limitation](#limitation) for the shapes it does not cover.
    ///
    /// It shadows [`EnrichFutureExt::enrich`] because a candidate type's
    /// inherent methods are searched before the traits it implements
    /// ([reference](https://doc.rust-lang.org/reference/expressions/method-call-expr.html#r-expr.method.candidate-search)).
    ///
    /// # Limitation
    ///
    /// This makes the hand-written shape work; it is **not** a general guarantee
    /// that the two wrappers commute. The re-ordering applies only when the
    /// receiver's statically-known type is exactly `Transferred<_>` and its
    /// inner type is not itself a `Transferred<_>`.
    ///
    /// In other wrapper shapes the blanket trait impl may be selected instead,
    /// or the re-ordering may not reach deep enough. Entries are unsupported
    /// there and may be lost when the enrichment and transfer operate on the
    /// same captured sink slot:
    ///
    /// - explicit trait dispatch, `EnrichFutureExt::enrich(fut.attach(t), &sink, e)`;
    /// - generic code whose receiver is only known as `F: EnrichFutureExt`,
    ///   since the concrete `Transferred` type is not visible there;
    /// - a boxed or type-erased receiver, `Box::pin(fut.attach(t)).enrich(...)`.
    ///   `Pin<Box<Transferred<_>>>` is itself a future, so the trait candidate
    ///   wins before `Transferred` is reached;
    /// - nested transfers, `.attach(t1).attach(t2).enrich(e)`. This method *is*
    ///   selected here, but it re-orders exactly one wrapper deep: the inner
    ///   call resolves through the trait because `T` is a type parameter there,
    ///   so the innermost transfer still replaces the chain.
    ///
    /// Note that those are the shapes an *integration* naturally produces -
    /// `observed_rt` boxes the future it attaches to, and middleware may attach
    /// around an already-attached future. So do not build on this method when
    /// writing a spawner or layer: put the entries in the transfer with
    /// [`Transfer::with_enrichment`](crate::context::Transfer::with_enrichment),
    /// which is order-independent and survives boxing and re-wrapping. For
    /// *targeted* entries use
    /// [`Transfer::with_enrichment_for`](crate::context::Transfer::with_enrichment_for)
    /// instead, as described on [`enrich_for`](Self::enrich_for). Enriching
    /// before attaching also works.
    ///
    /// `DESIGN.md` records this contract and the method-resolution mechanics
    /// behind it.
    pub fn enrich(self, sink: &Sink, enrichment: impl Enrichment) -> Transferred<Enriched<T>> {
        Transferred::new(self.inner.enrich(sink, enrichment), self.context_transfer)
    }

    /// Wraps this future so that `enrichment` is active for `target` only.
    ///
    /// The entries are active on every poll as targeted enrichments, layered on
    /// top of the transferred context.
    ///
    /// Re-orders the wrappers exactly as [`enrich`](Self::enrich) does, and is
    /// selected under the same rule, so the shapes listed in its
    /// [Limitation](Self::enrich) section drop these entries too.
    ///
    /// The escape hatch differs, though.
    /// [`Transfer::with_enrichment`](crate::context::Transfer::with_enrichment)
    /// is **not** a substitute here: it produces untargeted entries, so a
    /// targeted entry routed through it would be widened to every non-isolated
    /// sink in the captured transfer.
    /// Use
    /// [`Transfer::with_enrichment_for`](crate::context::Transfer::with_enrichment_for),
    /// which preserves the target and is likewise order-independent, or enrich
    /// before attaching.
    pub fn enrich_for(self, sink: &Sink, target: SinkId, enrichment: impl Enrichment) -> Transferred<Enriched<T>> {
        Transferred::new(self.inner.enrich_for(sink, target, enrichment), self.context_transfer)
    }
}

impl<F: Future> Future for Transferred<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.context_transfer.apply_current_thread();
        this.inner.poll(cx)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::future::Ready;

    use static_assertions::assert_impl_all;

    use super::*;

    // `Transferred` is held across `.await` points, so losing `Send` would stop
    // it working with multi-threaded executors. It is `Send` structurally, which
    // is exactly why the loss would be silent: adding a non-`Send` field here or
    // to `Transfer` breaks it with no signal at this definition.
    assert_impl_all!(Transferred<Ready<()>>: Send);
}
