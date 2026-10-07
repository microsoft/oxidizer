// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Integration tests verifying that enrichment context propagates automatically
//! through all spawn paths when the runtime is built with `.sink()`.

testing_aids::init_tracing!();

mod support;

use arty::runtime::Runtime;
#[cfg(feature = "macros")]
use arty::task::Builtins;
use observed::enrichment::EnrichFutureExt;
use observed::{Enrichment, Sink};
use support::JoinHandleExt as _;
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
#[cfg(feature = "macros")]
struct OuterCtx {
    outer: UnclassifiedI64,
}

#[cfg(feature = "macros")]
impl OuterCtx {
    fn new(outer: i64) -> Self {
        Self { outer: outer.into() }
    }
}

#[derive(Enrichment)]
#[cfg(feature = "macros")]
struct InnerCtx {
    inner: UnclassifiedI64,
}

#[cfg(feature = "macros")]
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

fn runtime_with_emitter(sink: &Sink) -> Runtime {
    Runtime::builder().sink(sink.clone()).build().expect("Failed to create runtime")
}

#[cfg(feature = "macros")]
fn collect_enrichment_keys(sink: &Sink) -> Vec<String> {
    sink.current_enrichments()
        .into_iter()
        .map(|e| e.key().as_str().to_owned())
        .collect()
}

/// Enrichment propagates through `Scheduler::spawn`.
#[cfg(feature = "macros")]
#[arty::test]
async fn enrichment_propagates_via_scheduler_spawn(cx: Builtins) {
    let sink = cx.sink().clone();
    let result = async {
        let handle = cx.scheduler().spawn({
            let sink = sink.clone();
            async move |_cx: Builtins| collect_enrichment_keys(&sink)
        });
        handle.await.unwrap()
    }
    .enrich(&sink, RequestCtx::new(42))
    .await;

    assert_eq!(result, ["request.id"]);
}

/// Enrichment propagates through `Scheduler::spawn_anywhere`.
#[cfg(feature = "macros")]
#[arty::test]
async fn enrichment_propagates_via_spawn_anywhere(cx: Builtins) {
    let sink = cx.sink().clone();
    let result = async {
        let handle = cx
            .scheduler()
            .spawn_anywhere(sink.clone(), |sink: Sink| async move { collect_enrichment_keys(&sink) });
        handle.await.unwrap()
    }
    .enrich(&sink, RequestCtx::new(99))
    .await;

    assert_eq!(result, ["request.id"]);
}

/// Enrichment propagates through `LocalScheduler::spawn`.
#[cfg(feature = "macros")]
#[arty::test]
async fn enrichment_propagates_via_local_scheduler_spawn(cx: Builtins) {
    let sink = cx.sink().clone();
    let result = async {
        let local = cx.local_scheduler().expect("should be on the correct thread");
        let handle = local.spawn({
            let sink = sink.clone();
            async move || collect_enrichment_keys(&sink)
        });
        handle.await.unwrap()
    }
    .enrich(&sink, RequestCtx::new(1))
    .await;

    assert_eq!(result, ["request.id"]);
}

/// No enrichments leak when none are set at the spawn site.
#[cfg(feature = "macros")]
#[arty::test]
async fn no_enrichment_leak_without_context(cx: Builtins) {
    let sink = cx.sink().clone();
    // No .enrich() here — spawn directly.
    let handle = cx.scheduler().spawn({
        let sink = sink.clone();
        async move |_cx: Builtins| collect_enrichment_keys(&sink)
    });
    let result = handle.await.unwrap();

    assert!(result.is_empty());
}

/// Nested spawn preserves the full enrichment chain.
#[cfg(feature = "macros")]
#[arty::test]
async fn nested_spawn_preserves_enrichment_chain(cx: Builtins) {
    let sink = cx.sink().clone();
    let mut result = async {
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
    .await;

    // Level-2 should see both outer and inner enrichments.
    result.sort();
    assert_eq!(result, ["inner", "outer"]);
}

#[test]
fn task_outcomes_keep_the_submission_context() {
    for local in [false, true] {
        for panics in [false, true] {
            let (sink, processor) = observed_testing::test_emitter(observed_testing::TEST_ID);
            let runtime = runtime_with_emitter(&sink);
            let outcome = runtime
                .scheduler()
                .spawn_anywhere(
                    Unaware((sink.clone(), local, panics)),
                    |cx, Unaware((sink, local, panics))| async move {
                        async {
                            if local {
                                let task = cx.local_scheduler().unwrap().spawn(async move || {
                                    assert!(!panics, "local task panic");
                                });
                                task.await
                            } else {
                                let task = cx.scheduler().spawn(async move |_| {
                                    assert!(!panics, "remote task panic");
                                });
                                task.await
                            }
                        }
                        .enrich(&sink, RequestCtx::new(42))
                        .await
                        .map_err(Unaware)
                    },
                )
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
}
