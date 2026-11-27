// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::poll_fn;
use std::sync::mpsc;
use std::task::Poll;

use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, Policy};
use seismograph::snapshot::SnapshotOptions;
use seismograph_runtime::snapshot::{RuntimeState, WorkerState};

use crate::runtime::policy::{MonitorPolicy, WorkersPolicy};
use crate::runtime::{Runtime, RuntimeBuilder};

struct RecorderGuard;

impl RecorderGuard {
    fn new() -> Self {
        seismograph::recorder(Configuration::default());
        Self
    }
}

impl Drop for RecorderGuard {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

fn runtime(name: &str) -> Runtime {
    RuntimeBuilder::new()
        .name(name)
        .monitor(MonitorPolicy::enabled())
        .workers(WorkersPolicy::exactly(1))
        .build()
        .unwrap()
}

fn events(runtime: seismograph::recorder::runtime::RuntimeId) -> Vec<seismograph::recorder::event::Event> {
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    seismograph::snapshot::decode(bytes.as_bytes())
        .unwrap()
        .events
        .events
        .into_iter()
        .filter(|event| event.runtime().is_some_and(|payload| payload.runtime_id == runtime))
        .collect()
}

#[test]
fn enabled_monitoring_registers_runtime_and_workers() {
    let _monitoring = super::TEST_LOCK.lock().unwrap();
    let _recording = RecorderGuard::new();
    let runtime = runtime("registered-workers");
    let monitoring = runtime.monitoring();
    let active = monitoring.capture();

    assert_eq!(active.name, "registered-workers");
    assert_eq!(active.state, RuntimeState::Running);
    assert!(active.workers.iter().any(|worker| worker.state != WorkerState::Stopped));

    runtime.stop().unwrap();
    let stopped = monitoring.capture();
    assert_eq!(stopped.state, RuntimeState::Stopped);
}

#[test]
fn older_tasks_emit_poll_events_only_after_they_are_polled() {
    let _monitoring = super::TEST_LOCK.lock().unwrap();
    let _recording = RecorderGuard::new();
    let runtime = runtime("late-recording");
    let runtime_id = runtime.monitoring().id();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (wake, waker) = mpsc::channel();
    let (polled, polls) = mpsc::channel();
    let mut first = true;
    let active = scheduler.spawn(move |_| {
        poll_fn(move |cx| {
            if first {
                first = false;
                wake.send(cx.waker().clone()).unwrap();
            } else {
                polled.send(()).unwrap();
            }
            Poll::<()>::Pending
        })
    });
    let dormant = scheduler.spawn(|_| std::future::pending::<()>());
    let waker = waker.recv().unwrap();

    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    waker.wake();
    polls.recv().unwrap();

    let events = events(runtime_id);
    assert_eq!(events.iter().filter(|event| event.kind == EventKind::TaskPollStarted).count(), 1);
    assert_eq!(events.iter().filter(|event| event.kind == EventKind::TaskPollFinished).count(), 1);
    assert!(
        !events
            .iter()
            .any(|event| { matches!(event.kind, EventKind::TaskEnqueued | EventKind::TaskMaterialized) })
    );

    runtime.stop().unwrap();
    assert!(active.join().unwrap_err().is_shutdown());
    assert!(dormant.join().unwrap_err().is_shutdown());
}

#[test]
fn newly_recorded_async_tasks_emit_the_complete_lifecycle() {
    let _monitoring = super::TEST_LOCK.lock().unwrap();
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = runtime("complete-lifecycle");
    let runtime_id = runtime.monitoring().id();

    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42 }).join().unwrap(), 42);
    let events = events(runtime_id);

    for kind in [
        EventKind::TaskEnqueued,
        EventKind::TaskMaterialized,
        EventKind::TaskPollStarted,
        EventKind::TaskPollFinished,
    ] {
        assert!(events.iter().any(|event| event.kind == kind), "missing {kind:?}");
    }
    runtime.stop().unwrap();
}

#[test]
fn blocking_tasks_use_the_execution_worker_for_terminal_events() {
    let _monitoring = super::TEST_LOCK.lock().unwrap();
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = runtime("blocking-lifecycle");
    let runtime_id = runtime.monitoring().id();

    assert_eq!(runtime.scheduler().spawn_blocking(|| 42).join().unwrap(), 42);
    let events = events(runtime_id);
    let materialized = events.iter().find(|event| event.kind == EventKind::TaskMaterialized).unwrap();
    let completed = events.iter().find(|event| event.kind == EventKind::TaskCompleted).unwrap();

    assert_eq!(materialized.runtime().unwrap().worker_id, completed.runtime().unwrap().worker_id);
    runtime.stop().unwrap();
}
