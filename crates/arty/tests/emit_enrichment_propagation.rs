// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Integration tests verifying that enrichment context propagates automatically
//! through all spawn paths when the runtime is built with `.sink()`.

mod support;

use std::future::{pending, poll_fn};
use std::sync::mpsc;
use std::task::Poll;

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Builtins;
use observed::enrichment::EnrichFutureExt;
use observed::{Enrichment, Sink, emit, event};
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

#[derive(Enrichment)]
struct RequestCtx {
    #[dimension(log = "request.id")]
    request_id: UnclassifiedI64,
}

impl RequestCtx {
    fn new(request_id: i64) -> Self {
        Self {
            request_id: request_id.into(),
        }
    }
}

#[derive(Enrichment)]
struct OuterCtx {
    outer: UnclassifiedI64,
}

impl OuterCtx {
    fn new(outer: i64) -> Self {
        Self { outer: outer.into() }
    }
}

#[derive(Enrichment)]
struct InnerCtx {
    inner: UnclassifiedI64,
}

impl InnerCtx {
    fn new(inner: i64) -> Self {
        Self { inner: inner.into() }
    }
}

#[derive(Clone, Copy, Debug)]
struct UnclassifiedI64(i64);

impl From<i64> for UnclassifiedI64 {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl data_privacy::RedactedDisplay for UnclassifiedI64 {
    fn fmt(&self, redactor: &dyn data_privacy::Redactor, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        const DATA_CLASS: data_privacy::DataClass = data_privacy::DataClass::new("unclassified", "i64");
        if data_privacy::Redactor::redacts(redactor, &DATA_CLASS) {
            let value = self.0.to_string();
            data_privacy::Redactor::redact(redactor, &DATA_CLASS, &value, f)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[event("test.task.drop")]
#[info]
struct TaskDrop;

struct EmitTaskDrop(Sink);

impl Drop for EmitTaskDrop {
    fn drop(&mut self) {
        emit!(self.0, TaskDrop);
    }
}

fn runtime_with_emitter(sink: &Sink) -> Runtime {
    Runtime::builder().sink(sink.clone()).build().expect("Failed to create runtime")
}

fn collect_enrichment_keys(sink: &Sink) -> Vec<String> {
    sink.current_enrichments()
        .into_iter()
        .map(|e| e.key().as_str().to_owned())
        .collect()
}

#[test]
fn default_sink_disables_enrichment() {
    let runtime = Runtime::builder().workers(WorkersPolicy::at_most(1)).build().unwrap();
    runtime
        .scheduler()
        .block_on(async |cx| {
            let sink = cx.sink();
            assert_eq!(sink.id(), Sink::noop().id());
            assert!(sink.is_noop());
            async {
                assert!(collect_enrichment_keys(sink).is_empty());
                let keys = cx
                    .scheduler()
                    .spawn(async |child| collect_enrichment_keys(child.sink()))
                    .await
                    .unwrap();
                assert!(keys.is_empty());
            }
            .enrich(sink, RequestCtx::new(42))
            .await;
        })
        .unwrap();
}

/// Enrichment propagates through `Scheduler::spawn`.
#[test]
fn enrichment_propagates_via_scheduler_spawn() {
    let sink = Sink::builder("test").build();
    let runtime = runtime_with_emitter(&sink);
    let result = runtime
        .scheduler()
        .block_on(async |cx| {
            async {
                let handle = cx.scheduler().spawn({
                    let sink = sink.clone();
                    async move |_cx: Builtins| collect_enrichment_keys(&sink)
                });
                handle.await.unwrap()
            }
            .enrich(&sink, RequestCtx::new(42))
            .await
        })
        .unwrap();

    assert_eq!(result, ["request.id"]);
}

/// Enrichment propagates through `Scheduler::spawn_anywhere`.
#[test]
fn enrichment_propagates_via_spawn_anywhere() {
    let sink = Sink::builder("test").build();
    let runtime = runtime_with_emitter(&sink);
    let result = runtime
        .scheduler()
        .block_on(async |cx| {
            async {
                let handle = cx
                    .scheduler()
                    .spawn_anywhere(sink.clone(), |sink: Sink| async move { collect_enrichment_keys(&sink) });
                handle.await.unwrap()
            }
            .enrich(&sink, RequestCtx::new(99))
            .await
        })
        .unwrap();

    assert_eq!(result, ["request.id"]);
}

/// No enrichments leak when none are set at the spawn site.
#[test]
fn no_enrichment_leak_without_context() {
    let sink = Sink::builder("test").build();
    let runtime = runtime_with_emitter(&sink);
    let result = runtime
        .scheduler()
        .block_on(async |cx| {
            // No .enrich() here — spawn directly.
            let handle = cx.scheduler().spawn({
                let sink = sink.clone();
                async move |_cx: Builtins| collect_enrichment_keys(&sink)
            });
            handle.await.unwrap()
        })
        .unwrap();

    assert!(result.is_empty());
}

/// Nested spawn preserves the full enrichment chain.
#[test]
fn nested_spawn_preserves_enrichment_chain() {
    let sink = Sink::builder("test").build();
    let runtime = runtime_with_emitter(&sink);
    let mut result = runtime
        .scheduler()
        .block_on(async |cx| {
            async {
                // Spawn level-1 task.
                let handle = cx.scheduler().spawn({
                    let level_one_sink = sink.clone();
                    async move |cx: Builtins| {
                        // Level-1 adds its own enrichment and spawns level-2.
                        async {
                            let handle = cx.scheduler().spawn({
                                let level_two_sink = level_one_sink.clone();
                                async move |_cx: Builtins| collect_enrichment_keys(&level_two_sink)
                            });
                            handle.await.unwrap()
                        }
                        .enrich(&level_one_sink, InnerCtx::new(2))
                        .await
                    }
                });
                handle.await.unwrap()
            }
            .enrich(&sink, OuterCtx::new(1))
            .await
        })
        .unwrap();

    // Level-2 should see both outer and inner enrichments.
    result.sort();
    assert_eq!(result, ["inner", "outer"]);
}

#[test]
fn task_outcomes_keep_the_submission_context() {
    for panics in [false, true] {
        let (sink, processor) = observed_testing::test_emitter(observed_testing::TEST_ID);
        let runtime = runtime_with_emitter(&sink);
        let outcome = runtime
            .scheduler()
            .spawn_anywhere(Unaware((sink.clone(), panics)), |cx, Unaware((sink, panics))| async move {
                async {
                    let task = cx.scheduler().spawn(async move |_| {
                        assert!(!panics, "remote task panic");
                    });
                    task.await
                }
                .enrich(&sink, RequestCtx::new(42))
                .await
                .map_err(Unaware)
            })
            .join()
            .unwrap();
        assert_eq!(outcome.is_err(), panics);
        if let Err(Unaware(error)) = outcome {
            assert!(error.is_panic());
        }
        drop(runtime);
        let expected_name = if panics {
            "arty.rt.task.panicked"
        } else {
            "arty.rt.task.succeeded"
        };
        let correlated = processor
            .events()
            .into_iter()
            .filter(|event| {
                event.name() == expected_name
                    && event
                        .dimensions()
                        .iter()
                        .any(|(key, value)| key == "request.id" && value == &observed::Value::from("42"))
            })
            .count();
        assert_eq!(correlated, 1);
    }
}

#[test]
fn cancelled_task_destructor_keeps_the_submission_context() {
    let (sink, processor) = observed_testing::test_emitter(observed_testing::TEST_ID);
    let runtime = runtime_with_emitter(&sink);
    let (started, ready) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((sink, started)), |cx, Unaware((sink, started))| {
            let task_sink = sink.clone();
            let mut factory = Some((cx, task_sink, started));
            poll_fn(move |_| {
                let (cx, task_sink, started) = factory.take().expect("the factory future completes on its first poll");
                Poll::Ready(cx.scheduler().spawn(async move |_| {
                    let _drop_event = EmitTaskDrop(task_sink);
                    started.send(()).unwrap();
                    pending::<()>().await;
                }))
            })
            .enrich(&sink, RequestCtx::new(42))
        })
        .join()
        .unwrap();
    ready.recv_timeout(TEST_TIMEOUT).unwrap();

    support::runtime_control(&runtime).request_stop();
    runtime.stop().unwrap();

    assert!(task.join().unwrap_err().is_shutdown());
    let events = processor.events();
    let dropped = events.iter().find(|event| event.name() == "test.task.drop").unwrap();
    assert!(
        dropped
            .dimensions()
            .iter()
            .any(|(key, value)| key == "request.id" && value == &observed::Value::from("42"))
    );
}
