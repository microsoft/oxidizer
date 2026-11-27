// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests sink integration through the public extension API.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

use std::future::poll_fn;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;
use std::thread::ThreadId;

use arty::core::{Thread, ThreadAware};
use arty::runtime::Runtime;
use arty::runtime::hook::{HookFn, TaskObserver, TaskSpawn};
use arty::runtime::policy::WorkersPolicy;
use observed::Sink;
use observed::context::{Transfer, TransferGuard};
use observed::enrichment::{EnrichFnExt, EnrichFutureExt, Enrichment, EnrichmentEntry};
use observed_arty::{ObservedExt as _, ObservedOptions};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use testing_aids::execute_or_abandon;

assert_impl_all!(ObservedOptions: Clone, Send, Sync);
// Sink processors and samplers can contain state that is not unwind-safe.
assert_not_impl_any!(ObservedOptions: UnwindSafe, RefUnwindSafe);
assert_not_impl_any!(ObservedOptions: Copy);

/// Adds an observable marker to a sink's current enrichment chain.
struct Layer(&'static str, i64);

impl Enrichment for Layer {
    fn into_entries(self) -> Vec<EnrichmentEntry> {
        vec![EnrichmentEntry::unclassified(self.0, self.1)]
    }
}

/// Creates a regular sink whose enrichment state drives the public API tests.
fn sink() -> Sink {
    Sink::builder("observed_arty_test").build()
}

/// Returns a stable snapshot of the sink's active enrichment keys.
fn active_keys(sink: &Sink) -> Vec<String> {
    sink.current_enrichments().iter().map(|entry| entry.key().to_string()).collect()
}

/// Records one restoration boundary and the context visible at that boundary.
#[derive(Debug, Eq, PartialEq)]
struct RestorationObservation {
    thread: ThreadId,
    outcome: Poll<()>,
    keys: Vec<String>,
}

/// Shares restoration observations between Arty workers and the test thread.
#[derive(Clone, Default)]
struct RestorationLog(Arc<Mutex<Vec<RestorationObservation>>>);

impl RestorationLog {
    /// Takes the recorded observations after every expected poll completes.
    fn take(&self) -> Vec<RestorationObservation> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Builds a probe with a relocatable baseline worker context.
#[derive(Clone)]
struct RestorationProbeFactory {
    sink: Sink,
    worker_context: Transfer,
    observations: RestorationLog,
}

impl ThreadAware for RestorationProbeFactory {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.sink.relocate(source, destination);
        self.worker_context.relocate(source, destination);
    }
}

/// Applies a known worker baseline and records the adapter's restoration result.
struct RestorationProbe {
    /// Supplies the sink whose active keys the probe records.
    sink: Sink,
    /// Holds the transferable worker baseline that the probe applies.
    worker_context: Transfer,
    /// Keeps the worker baseline active until the observation is complete.
    guard: Option<TransferGuard>,
    /// Stores observations across worker threads.
    observations: RestorationLog,
}

impl RestorationProbe {
    /// Moves the factory state into one worker-local probe.
    fn new(factory: RestorationProbeFactory) -> Self {
        Self {
            sink: factory.sink,
            worker_context: factory.worker_context,
            guard: None,
            observations: factory.observations,
        }
    }
}

impl TaskObserver for RestorationProbe {
    fn poll_start(&mut self) {
        self.guard = Some(self.worker_context.apply_current_thread());
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        let observation = RestorationObservation {
            thread: std::thread::current().id(),
            outcome,
            keys: active_keys(&self.sink),
        };
        self.observations.0.lock().unwrap_or_else(PoisonError::into_inner).push(observation);
        self.guard = None;
    }
}

/// Installs the probe only after a parent context starts a tracked task.
fn restoration_probe(factory: RestorationProbeFactory) -> HookFn<TaskSpawn, RestorationProbeFactory> {
    HookFn::new(factory, |event, factory| {
        if active_keys(&factory.sink) == ["parent"] {
            event.observe_with(factory.clone(), RestorationProbe::new);
        }
    })
}

#[test]
fn options_convert_from_sink() {
    let sink = sink();
    let owned = ObservedOptions::from(sink.clone());
    let borrowed = ObservedOptions::from(&sink);

    (|| {
        assert_eq!(active_keys(owned.sink()), ["retained"]);
        assert_eq!(active_keys(borrowed.sink()), ["retained"]);
    })
    .enrich(&sink, Layer("retained", 1))();
}

#[test]
fn builder_transfers_nested_enrichment_to_another_worker_and_restores_context() {
    let sink = sink();
    let observations = RestorationLog::default();
    let worker_context = (|| sink.transfer_context()).enrich(&sink, Layer("worker", 0))();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        // Start hooks run in registration order and end hooks run in reverse.
        // The probe must observe after Observed releases its task guard.
        .on(restoration_probe(RestorationProbeFactory {
            sink: sink.clone(),
            worker_context,
            observations: observations.clone(),
        }))
        .enable_observed(sink.clone())
        .build()
        .unwrap();
    let (scheduler, source, target) = runtime
        .scheduler()
        .block_on(async |cx| {
            let mut workers = cx.control().workers();
            (cx.scheduler().clone(), workers.next().unwrap(), workers.next().unwrap())
        })
        .unwrap();
    assert_ne!(source.id(), target.id());

    let parent = scheduler.spawn_at(
        &source,
        (scheduler.clone(), target.clone(), sink),
        |(scheduler, target, sink)| async move {
            async {
                scheduler
                    .spawn_at(&target, sink.clone(), |sink| {
                        let mut polls = 0;
                        {
                            let sink = sink.clone();
                            poll_fn(move |cx| {
                                assert_eq!(active_keys(&sink), ["parent", "child"]);
                                polls += 1;
                                if polls == 1 {
                                    cx.waker().wake_by_ref();
                                    Poll::Pending
                                } else {
                                    Poll::Ready(())
                                }
                            })
                        }
                        .enrich(&sink, Layer("child", 2))
                    })
                    .await
                    .unwrap();
            }
            .enrich(&sink, Layer("parent", 1))
            .await;
        },
    );
    runtime.scheduler().block_on(async move |_| parent.await).unwrap().unwrap();

    assert_eq!(
        observations.take(),
        [
            RestorationObservation {
                thread: target.id(),
                outcome: Poll::Pending,
                keys: vec!["worker".to_owned()],
            },
            RestorationObservation {
                thread: target.id(),
                outcome: Poll::Ready(()),
                keys: vec!["worker".to_owned()],
            },
        ]
    );

    stop_bounded(runtime);
}

#[test]
fn external_submission_transfers_enrichment_and_preserves_caller_context() {
    let sink = sink();
    let runtime = Runtime::builder().enable_observed(&sink).build().unwrap();

    (|| {
        assert_eq!(active_keys(&sink), ["external"]);
        let task = runtime
            .scheduler()
            .spawn_anywhere(sink.clone(), |_, sink| async move { active_keys(&sink) });

        assert_eq!(futures::executor::block_on(task).unwrap(), ["external"]);
        assert_eq!(active_keys(&sink), ["external"]);
    })
    .enrich(&sink, Layer("external", 1))();

    assert!(active_keys(&sink).is_empty());
    stop_bounded(runtime);
}

#[test]
fn runtime_control_enables_transfer_for_later_local_tasks() {
    let sink = sink();
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();

    runtime
        .scheduler()
        .block_on(async |cx| {
            cx.control().enable_observed(sink.clone());
            async {
                cx.scheduler()
                    .spawn({
                        let sink = sink.clone();
                        move |_| async move {
                            assert_eq!(active_keys(&sink), ["control"]);
                        }
                    })
                    .await
                    .unwrap();
            }
            .enrich(&sink, Layer("control", 1))
            .await;
        })
        .unwrap();

    stop_bounded(runtime);
}

/// Applies the shared watchdog policy to runtime shutdown.
fn stop_bounded(runtime: Runtime) {
    execute_or_abandon(move || runtime.stop()).unwrap().unwrap();
}
