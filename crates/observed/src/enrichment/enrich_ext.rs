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
    /// Chain with [`.enrich()`](EnrichFutureExt::enrich) to also push
    /// per-scope entries on top of the transferred context.
    ///
    /// WARN: calling `enrich/enrich_for` after attach won't work because attach replaces
    ///       each captured slot's chain rather than layering on top of it.
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
