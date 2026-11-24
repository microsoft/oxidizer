// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests sink integration through the public extension API.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

mod support;

use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::thread::ThreadId;

use arty::core::{Thread, ThreadAware};
use arty::runtime::Runtime;
use arty::runtime::hook::{HookFn, TaskObserver, TaskSpawn};
use arty::runtime::policy::WorkersPolicy;
use observed::context::{Transfer, TransferGuard};
use observed::enrichment::{EnrichFnExt, EnrichFutureExt, Enrichment, EnrichmentEntry};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{FlushError, Sink};
use observed_arty::{ObservedExt as _, ObservedOptions};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use support::stop_bounded;

assert_impl_all!(ObservedOptions: Clone, Send, Sync);
assert_not_impl_any!(ObservedOptions: Copy);

struct IgnoreEvents;

impl EventProcessor for IgnoreEvents {
    fn is_interested(&self, _description: &EventDescription) -> bool {
        false
    }

    fn process(&self, _event: &EventView<'_>) {
        unreachable!("the processor rejects every event")
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

struct Layer(&'static str, i64);

impl Enrichment for Layer {
    fn into_entries(self) -> Vec<EnrichmentEntry> {
        vec![EnrichmentEntry::unclassified(self.0, self.1)]
    }
}

fn sink() -> Sink {
    Sink::builder("observed_arty_test").processors(vec![Arc::new(IgnoreEvents)]).build()
}

fn active_keys(sink: &Sink) -> Vec<String> {
    sink.current_enrichments().iter().map(|entry| entry.key().to_string()).collect()
}

#[derive(Debug, Eq, PartialEq)]
struct RestorationObservation {
    thread: ThreadId,
    outcome: Poll<()>,
    keys: Vec<String>,
}

#[derive(Clone, Default)]
struct RestorationLog(Arc<Mutex<Vec<RestorationObservation>>>);

impl RestorationLog {
    fn take(&self) -> Vec<RestorationObservation> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

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

struct RestorationProbe {
    sink: Sink,
    worker_context: Transfer,
    guard: Option<TransferGuard>,
    observations: RestorationLog,
}

impl RestorationProbe {
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
        if let Some(guard) = &mut self.guard {
            guard.apply_current_thread();
        } else {
            self.guard = Some(self.worker_context.apply_current_thread());
        }
    }

    fn poll_end(&mut self, outcome: Poll<()>) {
        self.observations
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(RestorationObservation {
                thread: std::thread::current().id(),
                outcome,
                keys: active_keys(&self.sink),
            });
        if let Some(guard) = &mut self.guard {
            guard.restore_current_thread();
        }
    }
}

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
    let options = ObservedOptions::from(sink.clone());

    assert_eq!(options.sink().id(), sink.id());
}

#[test]
fn builder_transfers_nested_enrichment_to_another_worker_and_restores_context() {
    let sink = sink();
    let observations = RestorationLog::default();
    let worker_context = (|| sink.transfer_context()).enrich(&sink, Layer("worker", 0))();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
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
            let mut workers = cx.control().workers().into_iter().cloned();
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
                        let task_sink = sink.clone();
                        let mut polls = 0;
                        poll_fn(move |cx| {
                            assert_eq!(active_keys(&task_sink), ["parent", "child"]);
                            polls += 1;
                            if polls == 1 {
                                cx.waker().wake_by_ref();
                                Poll::Pending
                            } else {
                                Poll::Ready(())
                            }
                        })
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
fn runtime_control_enables_transfer_for_later_local_tasks() {
    let sink = sink();
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();

    runtime
        .scheduler()
        .block_on(async |cx| {
            cx.control().enable_observed(sink.clone());
            async {
                let task_sink = sink.clone();
                cx.scheduler()
                    .spawn(move |_| async move {
                        assert_eq!(active_keys(&task_sink), ["control"]);
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
