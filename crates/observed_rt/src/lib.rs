// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

//! Context-propagating task spawner for [`observed`].
//!
//! Wraps [`anyspawn::Spawner`] so that every spawned task (async or blocking)
//! automatically inherits the current enrichment state from a given sink.
//!
//! See the [Enrichment](observed#enrichment) section in the `observed` crate for
//! background on how enrichment storage, scoping, and cross-thread transfer work.
//!
//! # Example
//!
//! ```ignore
//! use observed::enrichment::EnrichFutureExt;
//! use anyspawn::Spawner;
//!
//! let sink = Sink::new(APP, vec![Arc::new(pipeline)], tick::SimpleClock::new_system());
//!
//! async {
//!     let spawner = observed_rt::tokio(&sink);
//!     let handle = spawner.spawn(async {
//!         // enrichments from the spawn site are visible here
//!     });
//!     handle.await;
//! }
//! .enrich(&sink, [("request.id", "r-42")])
//! .await;
//! ```

#[cfg(any(feature = "tokio", test))]
use anyspawn::{BoxedBlockingTask, BoxedFuture};
#[cfg(any(feature = "tokio", test))]
use observed::enrichment::EnrichFutureExt;

/// Wraps a future so that it inherits the caller's context (enrichment) from
/// the given sink.
///
/// Designed as the `future_layer` argument for
/// [`anyspawn::CustomSpawnerBuilder::layer`].
#[cfg(any(feature = "tokio", test))]
#[must_use]
fn enrich_future(sink: &observed::Sink, fut: BoxedFuture) -> BoxedFuture {
    let transfer = sink.transfer_context();
    Box::pin(fut.attach(transfer))
}

/// Wraps a blocking task so that it inherits the caller's context (enrichment)
/// from the given sink.
///
/// Designed as the `blocking_layer` argument for
/// [`anyspawn::CustomSpawnerBuilder::layer`].
#[cfg(any(feature = "tokio", test))]
#[must_use]
fn enrich_fn(sink: &observed::Sink, task: BoxedBlockingTask) -> BoxedBlockingTask {
    let transfer = sink.transfer_context();
    Box::new(move || {
        let _guard = transfer.apply();
        task();
    })
}

/// Creates an [`anyspawn::Spawner`] that propagates [`observed`] enrichment context
/// to every spawned task (async and blocking).
///
/// Uses the Tokio runtime as the execution backend.
///
/// # Example
///
/// ```ignore
/// let spawner = observed_rt::tokio(&sink);
/// spawner.spawn(async { /* enrichments visible here */ });
/// ```
#[cfg(any(feature = "tokio", test))]
pub fn tokio(sink: &observed::Sink) -> anyspawn::Spawner {
    let e1 = sink.clone();
    let e2 = sink.clone();
    anyspawn::CustomSpawnerBuilder::tokio()
        .name("enrichment")
        .layer(move |fut| enrich_future(&e1, fut), move |t| enrich_fn(&e2, t))
        .build()
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, not(miri)))] // miri error: can't call foreign function `CreateIoCompletionPort` on OS `windows`
mod tests {
    use anyspawn::CustomSpawnerBuilder;
    use observed::{Enrichment, Sink};

    use super::*;

    #[derive(Enrichment)]
    struct RequestCtx {
        #[dimension(log = "request.id")]
        #[unredacted]
        request_id: i64,
    }

    #[derive(Enrichment)]
    struct TraceCtx {
        #[dimension(log = "trace.id")]
        #[unredacted]
        trace_id: i64,
    }

    #[derive(Enrichment)]
    struct InnerCtx {
        #[unredacted]
        inner: i64,
    }

    #[derive(Enrichment)]
    struct OuterCtx {
        #[unredacted]
        outer: i64,
    }

    #[tokio::test]
    async fn enrichments_propagate_to_spawned_task() {
        let sink = Sink::noop();
        let spawner_emitter = sink.clone();
        let spawner = {
            let e = spawner_emitter.clone();
            CustomSpawnerBuilder::tokio()
                .layer(move |fut| enrich_future(&e, fut), |t| t)
                .build()
        };

        let task_emitter = sink.clone();
        let enrichments = async { spawner.spawn(async move { task_emitter.current_enrichments() }).await }
            .enrich(&sink, RequestCtx { request_id: 42 })
            .await;

        assert_eq!(enrichments.len(), 1);
    }

    #[tokio::test]
    async fn enrichments_survive_yield_points() {
        let sink = Sink::noop();
        let spawner = {
            let e = sink.clone();
            CustomSpawnerBuilder::tokio()
                .layer(move |fut| enrich_future(&e, fut), |t| t)
                .build()
        };

        let task_emitter = sink.clone();
        let enrichments = async {
            spawner
                .spawn(async move {
                    tokio::task::yield_now().await;
                    task_emitter.current_enrichments()
                })
                .await
        }
        .enrich(&sink, TraceCtx { trace_id: 1 })
        .await;

        assert_eq!(enrichments.len(), 1);
    }

    #[tokio::test]
    async fn no_enrichments_without_context() {
        let sink = Sink::noop();
        let spawner = {
            let e = sink.clone();
            CustomSpawnerBuilder::tokio()
                .layer(move |fut| enrich_future(&e, fut), |t| t)
                .build()
        };

        let task_emitter = sink.clone();
        let enrichments = spawner.spawn(async move { task_emitter.current_enrichments() }).await;

        assert!(enrichments.is_empty());
    }

    #[tokio::test]
    async fn nested_enrichments_propagate() {
        let sink = Sink::noop();
        let spawner = {
            let e = sink.clone();
            CustomSpawnerBuilder::tokio()
                .layer(move |fut| enrich_future(&e, fut), |t| t)
                .build()
        };

        let task_emitter = sink.clone();
        let enrichments = async {
            async { spawner.spawn(async move { task_emitter.current_enrichments() }).await }
                .enrich(&sink, InnerCtx { inner: 2 })
                .await
        }
        .enrich(&sink, OuterCtx { outer: 1 })
        .await;

        assert_eq!(enrichments.len(), 2);
    }

    #[tokio::test]
    async fn enrichments_propagate_to_spawn_anywhere() {
        let sink = Sink::noop();
        let spawner = {
            let e = sink.clone();
            CustomSpawnerBuilder::tokio()
                .layer(move |fut| enrich_future(&e, fut), |t| t)
                .build()
        };

        let task_emitter = sink.clone();
        let enrichments = async {
            spawner
                .spawn_anywhere(task_emitter, |e| async move { e.current_enrichments() })
                .await
        }
        .enrich(&sink, RequestCtx { request_id: 99 })
        .await;

        assert_eq!(enrichments.len(), 1);
        assert_eq!(enrichments[0].key().as_str(), "request.id");
    }

    #[tokio::test]
    async fn enrichments_propagate_to_spawn_blocking() {
        let sink = Sink::noop();
        let spawner = tokio(&sink);

        let task_emitter = sink.clone();
        let enrichments = async { spawner.spawn_blocking(move || task_emitter.current_enrichments()).await }
            .enrich(&sink, RequestCtx { request_id: 42 })
            .await;

        assert_eq!(enrichments.len(), 1);
        assert_eq!(enrichments[0].key().as_str(), "request.id");
    }

    #[test]
    fn context_transfer_survives_thread_migration() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::Poll;

        use observed::enrichment::EnrichFnExt;
        use testing_aids::FutureTestExt;

        let sink = Sink::noop();
        let task_emitter = sink.clone();

        let yielded = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let capture_enrichment_fut: BoxedFuture = Box::pin(std::future::poll_fn(move |_cx| {
            tx.send(task_emitter.current_enrichments()).unwrap();
            if yielded.swap(true, Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }));

        let emitter_for_spawn = sink.clone();
        (move || {
            // capture RequestCtx enrichment
            let mut fut = enrich_future(&emitter_for_spawn, capture_enrichment_fut);
            assert!(testing_aids::poll_once(&mut fut).is_pending());
            // Second poll on a DIFFERENT thread, enrichments should persist across the yield point and thread migration
            std::thread::spawn(move || {
                fut.unwrap_ready();
            })
            .join()
            .unwrap();
        })
        .enrich(&sink, RequestCtx { request_id: 42 })();

        for _ in 0..2 {
            let enrichments = rx.try_recv().unwrap();
            let [entry] = &*enrichments else {
                panic!("expected exactly one enrichment entry");
            };
            assert_eq!(entry.key().as_str(), "request.id");
        }

        assert!(rx.try_recv().is_err(), "expected no more enrichment captures");
    }
}
