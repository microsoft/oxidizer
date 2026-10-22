// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Integration tests verifying that the runtime emits the documented `observed`
//! events with the correct names, dimensions, and dimension values.
//!
//! See `docs/observability.md` for the design. These tests assert that
//! our metadata is wired correctly through a real runtime, not that `observed`
//! itself works.

// Native runs use real processors; Miri uses the test-util processor model.

testing_aids::init_tracing!();

mod support;

use std::pin::Pin;
use std::task::{Context, Poll};

use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
#[cfg(all(debug_assertions, not(miri)))]
use many_cpus::SystemHardware;
use observed::Value;
use observed_testing::{CapturedEvent, TEST_ID, test_emitter};
use support::JoinHandleExt as _;
#[cfg(all(debug_assertions, not(miri)))]
use thread_aware::ThreadAware;
use thread_aware::Unaware;

const PROCESSORS: usize = 2;
const TASKS: usize = 5;

fn events_named<'a>(events: &'a [CapturedEvent], name: &str) -> Vec<&'a CapturedEvent> {
    events.iter().filter(|e| e.name() == name).collect()
}

fn dimension(event: &CapturedEvent, key: &str) -> Option<Value> {
    event.dimensions().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn poll_count(event: &CapturedEvent) -> f64 {
    event
        .field_metrics()
        .iter()
        .find(|metric| metric.field_key() == "polls")
        .expect("task terminal events include a poll-count histogram")
        .value()
}

struct CancellationPanicFuture {
    started: Option<std::sync::mpsc::Sender<()>>,
}

impl Future for CancellationPanicFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.started
            .take()
            .expect("the cancellation probe is polled once")
            .send(())
            .expect("the cancellation test keeps its receiver alive while polling");
        Poll::Pending
    }
}

impl Drop for CancellationPanicFuture {
    #[expect(clippy::panic, reason = "this test injects a destructor panic to verify telemetry")]
    fn drop(&mut self) {
        panic!("intentional cancellation destructor panic for telemetry test");
    }
}

#[test]
fn blocking_pool_reports_distinct_saturation_episodes() {
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::at_most(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .sink(sink)
        .build()
        .unwrap();

    for expected_events in 1..=2 {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = runtime.scheduler().spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();

        let queued: Vec<_> = (0..=5).map(|_| runtime.scheduler().spawn_blocking(|| {})).collect();
        release_tx.send(()).unwrap();
        blocker.join().unwrap();
        for task in queued {
            task.join().unwrap();
        }

        assert_eq!(
            events_named(&processor.events(), "arty.rt.blocking_worker.pool_saturated").len(),
            expected_events
        );
    }

    runtime.stop().unwrap();
}

#[cfg(all(debug_assertions, not(miri)))]
#[test]
fn validation_accepts_associated_worker() {
    let (sink, processor) = test_emitter(TEST_ID);
    Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .sink(sink)
        .build()
        .unwrap()
        .scheduler()
        .block_on(async |cx| {
            let _ = cx.thread();
        })
        .unwrap();

    assert_eq!(
        processor
            .events()
            .iter()
            .filter(|event| { event.name() == "arty.rt.builtins.thread_mismatch" })
            .count(),
        0,
    );
}

#[cfg(all(debug_assertions, not(miri)))]
#[test]
fn validation_follows_relocation_with_known_source() {
    validation_follows_accepted_worker(true);
}

#[cfg(all(debug_assertions, not(miri)))]
#[test]
fn validation_follows_relocation_with_unknown_source() {
    validation_follows_accepted_worker(false);
}

#[cfg(all(debug_assertions, not(miri)))]
#[cfg_attr(test, mutants::skip)]
fn validation_follows_accepted_worker(known_source: bool) {
    if SystemHardware::current().processors().len() < 2 {
        eprintln!("requires two runtime workers to exercise cross-worker validation");
        return;
    }
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .sink(sink)
        .build()
        .expect("two available processors are required to check cross-worker validation");
    let workers: Vec<_> = (0..2)
        .map(|_| {
            runtime.scheduler().spawn_anywhere((), |cx, ()| async move {
                let thread = cx.thread().clone();
                let scheduler = cx.scheduler().clone();
                (cx, scheduler, thread)
            })
        })
        .map(|handle| handle.join().expect("each worker must return its services"))
        .collect();
    let mut builtins = workers[0].0.clone();
    let source = workers[0].2.clone();
    workers[1]
        .1
        .spawn(async move |cx| {
            let _ = builtins.thread();
            builtins.relocate(known_source.then_some(&source), cx.thread());
            let _ = builtins.thread();
        })
        .join()
        .expect("relocation must complete on the destination worker");
    runtime.stop().expect("workers must shut down after validation");

    assert_eq!(
        processor
            .events()
            .iter()
            .filter(|event| { event.name() == "arty.rt.builtins.thread_mismatch" })
            .count(),
        1,
    );
}

#[test]
fn started_event_reports_worker_policy() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(PROCESSORS))
        .sink(sink)
        .build()
        .unwrap();

    runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap();
    runtime.stop().unwrap();

    let events = processor.events();
    let started = events_named(&events, "arty.rt.started");
    assert_eq!(started.len(), 1, "exactly one runtime should start");

    assert_eq!(dimension(started[0], "processors.used"), Some("2".into()));
    assert!(
        dimension(started[0], "processors.available").is_some(),
        "processors.available dimension should be present"
    );
    assert_eq!(dimension(started[0], "blocking_worker_pool.mode"), Some("shared".into()));
}

#[test]
fn each_async_worker_starts_and_stops() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(PROCESSORS))
        .sink(sink)
        .build()
        .unwrap();

    runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap();
    runtime.stop().unwrap();

    let events = processor.events();
    assert_eq!(events_named(&events, "arty.rt.async_worker.started").len(), PROCESSORS);
    assert_eq!(events_named(&events, "arty.rt.async_worker.stopped").len(), PROCESSORS);
    assert_eq!(events_named(&events, "arty.rt.stopping").len(), 1);
    assert_eq!(events_named(&events, "arty.rt.stopped").len(), 1);
}

#[test]
fn async_worker_os_threads_report_lifecycle() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(PROCESSORS))
        .sink(sink)
        .build()
        .unwrap();

    runtime.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap();
    runtime.stop().unwrap();

    let events = processor.events();
    assert_eq!(
        events_named(&events, "arty.rt.thread.spawn").len(),
        PROCESSORS,
        "each async worker OS thread is announced before spawning"
    );
    assert_eq!(
        events_named(&events, "arty.rt.thread.started").len(),
        PROCESSORS,
        "each async worker OS thread reports that it began running"
    );
    assert_eq!(
        events_named(&events, "arty.rt.thread.exiting").len(),
        PROCESSORS,
        "each async worker OS thread reports that it exited cleanly"
    );

    for event in events_named(&events, "arty.rt.thread.started") {
        assert!(dimension(event, "thread.name").is_some(), "a started thread carries its name");
        assert!(dimension(event, "arty.thread.id").is_some(), "a started thread carries its id");
    }
}

#[test]
fn spawned_task_emits_spawned_and_completed() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(PROCESSORS))
        .sink(sink)
        .build()
        .unwrap();

    let handles: Vec<_> = (0..TASKS)
        .map(|_| runtime.scheduler().spawn_anywhere((), |_, ()| async {}))
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    runtime.stop().unwrap();

    let events = processor.events();
    let spawned = events_named(&events, "arty.rt.task.spawned");
    assert!(spawned.len() >= TASKS);
    for event in &spawned {
        assert_eq!(dimension(event, "placement"), Some("any".into()));
    }
    assert!(events_named(&events, "arty.rt.task.succeeded").len() >= TASKS);
}

#[test]
fn same_worker_spawn_reports_same_thread_placement() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).sink(sink).build().unwrap();

    runtime
        .scheduler()
        .block_on(async |cx| {
            cx.scheduler().spawn(async |_| {}).await.unwrap();
        })
        .unwrap();
    runtime.stop().unwrap();

    let events = processor.events();
    assert!(
        events_named(&events, "arty.rt.task.spawned")
            .iter()
            .any(|event| dimension(event, "placement") == Some("same_thread".into()))
    );
}

#[test]
fn targeted_spawn_reports_specific_thread_placement() {
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).sink(sink).build().unwrap();

    runtime
        .scheduler()
        .block_on(async |cx| {
            let destination = cx.control().workers().into_iter().next().unwrap();
            cx.scheduler().spawn_at(destination, (), |()| async {}).await.unwrap();
        })
        .unwrap();
    runtime.stop().unwrap();

    assert!(
        events_named(&processor.events(), "arty.rt.task.spawned")
            .iter()
            .any(|event| dimension(event, "placement") == Some("specific_thread".into()))
    );
}

#[test]
fn panicking_task_emits_panicked_event() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(PROCESSORS))
        .sink(sink)
        .build()
        .unwrap();

    let handle = runtime
        .scheduler()
        .spawn_anywhere::<(), _, ()>((), |_, ()| async { panic!("intentional panic for telemetry test") });
    // The panic propagates through `wait()`; swallow it so the test thread survives.
    assert!(handle.join().unwrap_err().is_panic());
    runtime.stop().unwrap();

    let events = processor.events();
    let panicked = events_named(&events, "arty.rt.task.panicked");
    assert_eq!(panicked.len(), 1);
}

#[test]
fn successful_multi_poll_task_reports_aggregate_poll_count() {
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).sink(sink).build().unwrap();

    runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async {
            let mut remaining = 3;
            std::future::poll_fn(move |cx| {
                remaining -= 1;
                if remaining == 0 {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
            .await;
        })
        .join()
        .unwrap();
    runtime.stop().unwrap();

    assert!(
        events_named(&processor.events(), "arty.rt.task.succeeded")
            .into_iter()
            .any(|event| (poll_count(event) - 3.0).abs() < f64::EPSILON)
    );
}

#[test]
fn panicking_multi_poll_task_reports_aggregate_poll_count() {
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).sink(sink).build().unwrap();

    let error = runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async {
            let mut remaining = 3;
            std::future::poll_fn(move |cx| -> Poll<()> {
                remaining -= 1;
                assert!(remaining != 0, "intentional multi-poll panic for telemetry test");
                cx.waker().wake_by_ref();
                Poll::Pending
            })
            .await;
        })
        .join()
        .unwrap_err();
    assert!(error.is_panic());
    runtime.stop().unwrap();

    assert!(
        events_named(&processor.events(), "arty.rt.task.panicked")
            .into_iter()
            .any(|event| (poll_count(event) - 3.0).abs() < f64::EPSILON)
    );
}

#[test]
fn cancellation_destructor_panic_emits_panicked_event() {
    let (sink, processor) = test_emitter(TEST_ID);
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).sink(sink).build().unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let handle = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started_tx), |_, Unaware(started)| CancellationPanicFuture {
            started: Some(started),
        });
    started_rx.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();

    runtime.stop().unwrap();

    assert!(handle.join().unwrap_err().is_shutdown());
    assert!(
        events_named(&processor.events(), "arty.rt.task.panicked")
            .into_iter()
            .any(|event| (poll_count(event) - 1.0).abs() < f64::EPSILON)
    );
}

#[test]
fn round_robin_submissions_emit_one_spawn_event_per_worker() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(PROCESSORS))
        .sink(sink)
        .build()
        .unwrap();

    let handles: Vec<_> = (0..PROCESSORS)
        .map(|_| runtime.scheduler().spawn_anywhere((), |_, ()| async {}))
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    runtime.stop().unwrap();

    let events = processor.events();
    assert!(events_named(&events, "arty.rt.task.spawned").len() >= PROCESSORS);
}

#[test]
fn tasks_discarded_on_shutdown_do_not_emit_terminal_events() {
    let (sink, processor) = test_emitter(TEST_ID);

    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(PROCESSORS))
        .sink(sink)
        .build()
        .unwrap();

    // Spawn tasks that never complete, then tear down: no task completes or panics, so the
    // only terminal-state telemetry is the absence of completed/panicked.
    for _ in 0..TASKS {
        _ = runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { std::future::pending::<()>().await });
    }
    runtime.stop().unwrap();

    let events = processor.events();
    let spawned = events_named(&events, "arty.rt.task.spawned").len();
    let completed = events_named(&events, "arty.rt.task.succeeded").len();
    let panicked = events_named(&events, "arty.rt.task.panicked").len();
    assert_eq!(completed + panicked, 0, "pending tasks never reach a terminal state");
    assert!(spawned <= TASKS);
}
