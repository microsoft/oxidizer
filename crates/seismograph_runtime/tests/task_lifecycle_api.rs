// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public task lifecycle API contracts.

use std::sync::Mutex;

use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, Policy};
use seismograph::snapshot::SnapshotOptions;
use seismograph_runtime::{Runtime, TaskIdentity, TaskOutcome, WorkerMetadata, WorkerRole};

static RECORDING: Mutex<()> = Mutex::new(());

#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper follows the same unwrap policy as its test callers"
)]
fn runtime_events(runtime: seismograph::recorder::runtime::RuntimeId) -> Vec<seismograph::recorder::event::Event> {
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
fn async_queue_activation_poll_and_outcome_preserve_the_supplied_context() {
    let _recording = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = Runtime::builder("async-lifecycle").configured_workers(2).build();
    let origin = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    let executor = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    let parent = TaskIdentity::from_location(std::panic::Location::caller());
    let identity = TaskIdentity::from_location(std::panic::Location::caller());

    let queued = runtime
        .async_task(identity, 64)
        .unwrap()
        .with_context(Some(origin.origin()), Some(parent.id()))
        .enqueued(3);
    let task = queued.activated(&executor);
    let poll = task.poll_started(&executor);
    assert!(poll.finished(Some(TaskOutcome::Completed)).is_none());

    let events = runtime_events(runtime.id())
        .into_iter()
        .filter(|event| {
            matches!(
                event.kind,
                EventKind::TaskEnqueued
                    | EventKind::TaskMaterialized
                    | EventKind::TaskPollStarted
                    | EventKind::TaskPollFinished
                    | EventKind::TaskCanceled
            )
        })
        .collect::<Vec<_>>();
    seismograph::recorder(Configuration::default());

    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            EventKind::TaskEnqueued,
            EventKind::TaskMaterialized,
            EventKind::TaskPollStarted,
            EventKind::TaskPollFinished,
        ]
    );
    let queued = events[0].runtime().unwrap();
    assert_eq!(
        (
            queued.worker_id,
            queued.subject_id,
            queued.related_id,
            queued.value_0,
            queued.value_1,
        ),
        (
            Some(origin.id()),
            identity.id().get(),
            parent.id().get(),
            identity.descriptor().get(),
            3,
        )
    );
    let activated = events[1].runtime().unwrap();
    assert_eq!(
        (activated.worker_id, activated.subject_id, activated.value_1),
        (Some(executor.id()), identity.id().get(), 65)
    );
    let finished = events[3].runtime().unwrap();
    assert_eq!(
        (finished.worker_id, finished.subject_id, finished.value_1),
        (Some(executor.id()), identity.id().get(), 1)
    );
}

#[test]
fn dropping_queued_tasks_records_implicit_cancellation() {
    let _recording = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = Runtime::builder("implicit-cancellation").build();
    let async_identity = TaskIdentity::from_location(std::panic::Location::caller());
    let blocking_identity = TaskIdentity::from_location(std::panic::Location::caller());

    drop(runtime.async_task(async_identity, 0).unwrap().enqueued(7));
    drop(runtime.blocking_task(blocking_identity, 0).unwrap().enqueued(11));

    let events = runtime_events(runtime.id());
    seismograph::recorder(Configuration::default());

    assert!(events.iter().any(|event| {
        event.kind == EventKind::TaskCanceled
            && event
                .runtime()
                .is_some_and(|payload| payload.subject_id == async_identity.id().get())
    }));
    assert!(events.iter().any(|event| {
        event.kind == EventKind::TaskCanceled
            && event
                .runtime()
                .is_some_and(|payload| payload.subject_id == blocking_identity.id().get() && payload.worker_id.is_none())
    }));
}

#[test]
fn blocking_start_and_finish_record_queue_and_execution_context() {
    let _recording = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = Runtime::builder("blocking-lifecycle").configured_workers(2).build();
    let origin = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    let executor = runtime.blocking_worker(WorkerMetadata::new(WorkerRole::Blocking));
    let identity = TaskIdentity::from_location(std::panic::Location::caller());

    let queued = runtime
        .blocking_task(identity, 32)
        .unwrap()
        .with_context(Some(origin.origin()), None)
        .enqueued(5);
    let execution = queued.started(&executor);
    execution.finished(TaskOutcome::Panicked);

    let events = runtime_events(runtime.id())
        .into_iter()
        .filter(|event| {
            matches!(
                event.kind,
                EventKind::TaskEnqueued | EventKind::TaskMaterialized | EventKind::TaskPanicked
            )
        })
        .collect::<Vec<_>>();
    seismograph::recorder(Configuration::default());

    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [EventKind::TaskEnqueued, EventKind::TaskMaterialized, EventKind::TaskPanicked,]
    );
    let queued = events[0].runtime().unwrap();
    assert_eq!(
        (queued.worker_id, queued.subject_id, queued.value_0, queued.value_1),
        (Some(origin.id()), identity.id().get(), identity.descriptor().get(), 5)
    );
    let started = events[1].runtime().unwrap();
    assert_eq!(
        (started.worker_id, started.subject_id, started.value_1),
        (Some(executor.id()), identity.id().get(), 33)
    );
    let finished = events[2].runtime().unwrap();
    assert_eq!(
        (finished.worker_id, finished.subject_id, finished.value_1),
        (Some(executor.id()), identity.id().get(), 0)
    );
}
