// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment extension traits for futures and closures.

use std::any::type_name;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};

use super::entry::EnrichmentEntry;
use crate::context::{Transfer, Transferred};
use crate::enrichment::Enrichment;
use crate::{Sink, SinkId};

/// A future wrapper that pushes/pops enrichment on every poll.
///
/// Created by [`EnrichFutureExt::enrich`].
/// See the [Enrichment](crate#enrichment) section for the full model.
#[pin_project::pin_project]
#[must_use]
pub struct Enriched<T> {
    #[pin]
    inner: T,
    sink: Sink,
    entries: Arc<[EnrichmentEntry]>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for Enriched<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<F: Future> Future for Enriched<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _guard = this.sink.push_enrichment(Arc::clone(this.entries));
        this.inner.poll(cx)
    }
}

/// Extension trait that adds methods for enriching an async block of code.
pub trait EnrichFutureExt: Future + Sized {
    /// Wraps this future so that `enrichment` entries are active on every poll.
    fn enrich(self, sink: &Sink, enrichment: impl Enrichment) -> Enriched<Self> {
        let entries: Arc<[EnrichmentEntry]> = enrichment.into_entries().into();
        Enriched {
            inner: self,
            entries,
            sink: sink.clone(),
        }
    }

    /// Wraps this future so that `enrichment` entries are active as targeted enrichments
    /// for `target` on every poll.
    fn enrich_for(self, sink: &Sink, target: SinkId, enrichment: impl Enrichment) -> Enriched<Self> {
        let entries: Arc<[EnrichmentEntry]> = enrichment.into_entries().into_iter().map(move |e| e.with_target(target)).collect();
        Enriched {
            inner: self,
            entries,
            sink: sink.clone(),
        }
    }

    /// Wraps this future so that a captured
    /// [`Transfer`] is restored on every poll.
    ///
    /// This is integration plumbing. If you spawn through `observed_rt` or
    /// `oxidizer_rt`, enrichment already follows your tasks and you do not need
    /// this - reach for it when writing a spawner, a `tower` layer, or similar
    /// middleware yourself.
    ///
    /// To add entries of your own, prefer putting them in the [`Transfer`]
    /// via [`Transfer::with_enrichment`](crate::context::Transfer::with_enrichment)
    /// or
    /// [`Transfer::with_enrichment_for`](crate::context::Transfer::with_enrichment_for):
    /// that is independent of wrapper order, so it survives the future being
    /// boxed or wrapped again further out.
    ///
    /// Chaining [`.enrich()`](EnrichFutureExt::enrich) around the result also
    /// works for a single `attach` on a plain, non-boxed future -
    /// [`Transferred::enrich`](crate::context::Transferred::enrich) re-orders the
    /// wrappers - but that is a convenience for hand-written code, not a general
    /// guarantee. See its docs for the shapes it does not cover.
    fn attach(self, transfer: Transfer) -> Transferred<Self> {
        Transferred::new(self, transfer)
    }
}

impl<F: Future> EnrichFutureExt for F {}

/// Extension trait that adds methods for enriching a synchronous block of code.
pub trait EnrichFnExt<R>: FnOnce() -> R + Sized {
    /// Wraps this closure so that `enrichment` entries are active when called.
    fn enrich(self, sink: &Sink, enrichment: impl Enrichment) -> impl FnOnce() -> R {
        let entries: Arc<[EnrichmentEntry]> = enrichment.into_entries().into();
        let sink = sink.clone();
        move || {
            // For composite emitters, this broadcasts the push to every
            // child's slot; the returned compound guard pops them all on Drop.
            let _guard = sink.push_enrichment(entries);
            self()
        }
    }

    /// Wraps this closure so that `enrichment` entries are active as targeted enrichments
    /// for `target` when called.
    fn enrich_for(self, sink: &Sink, target: SinkId, enrichment: impl Enrichment) -> impl FnOnce() -> R {
        let entries: Arc<[EnrichmentEntry]> = enrichment.into_entries().into_iter().map(|e| e.with_target(target)).collect();
        let sink = sink.clone();
        move || {
            let _guard = sink.push_enrichment(entries);
            self()
        }
    }
}

impl<F, R> EnrichFnExt<R> for F where F: FnOnce() -> R {}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::future::Ready;

    use static_assertions::assert_impl_all;

    use super::*;

    // `Enriched` is held across `.await` points, so losing `Send` would stop it
    // working with multi-threaded executors. It is `Send` structurally, which is
    // exactly why the loss would be silent: adding a non-`Send` field here or to
    // `Sink` breaks it with no signal at this definition.
    assert_impl_all!(Enriched<Ready<()>>: Send);
}
