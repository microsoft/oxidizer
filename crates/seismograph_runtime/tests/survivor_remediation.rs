// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contracts at the boundary between native task providers and inspection.

use seismograph::recorder::event::EventTimestamp;
use seismograph_runtime::snapshot::{CaptureOptions, Task, TaskActivityState, TaskCollection, TaskMetrics, TaskMetricsScope};
use seismograph_runtime::{Runtime, TaskIdentity, WorkerMetadata, WorkerRole};

#[test]
fn providers_cannot_supply_poll_measurements_or_override_the_responding_worker() {
    let runtime = Runtime::builder("provider-measurements").configured_workers(2).build();
    let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    let other = runtime.async_worker(WorkerMetadata::new(WorkerRole::Io));
    let other_id = other.id();
    let first = TaskIdentity::from_location(std::panic::Location::caller());
    let second = TaskIdentity::from_location(std::panic::Location::caller());
    worker
        .register_task_provider(move |request| {
            let tasks = [second, first]
                .map(|identity| {
                    let mut task = Task::new(identity.id(), identity.descriptor(), TaskActivityState::Ready);
                    task.last_worker_id = Some(other_id);
                    task.metrics = TaskMetrics {
                        poll_count: 7,
                        poll_duration_nanos: 19,
                        ..TaskMetrics::default()
                    };
                    task.metrics_scope = TaskMetricsScope::PollsSince(EventTimestamp::from_ticks(3));
                    task
                })
                .to_vec();
            request.respond(tasks, TaskCollection::Complete);
        })
        .unwrap();

    let captured = runtime.capture(CaptureOptions::default()).unwrap();

    assert_eq!(
        captured.tasks.iter().map(|task| task.id).collect::<Vec<_>>(),
        [first.id(), second.id()]
    );
    for task in &captured.tasks {
        assert_eq!(task.last_worker_id, Some(worker.id()));
        assert_eq!((task.future_size_bytes, task.spawned_at), (None, None));
        assert_eq!(task.metrics, TaskMetrics::default());
        assert_eq!(task.metrics_scope, TaskMetricsScope::Uncollected);
    }
}

#[test]
fn explicit_stop_rejects_later_inspection_and_provider_registration() {
    let runtime = Runtime::builder("registration-retirement").configured_workers(1).build();
    let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    runtime.stop();
    assert_eq!(
        runtime.capture(CaptureOptions::default()).unwrap_err().to_string(),
        "cannot inspect a stopped runtime"
    );
    assert_eq!(
        worker.register_task_provider(drop).unwrap_err().to_string(),
        "cannot register a task provider on a stopped worker"
    );
}

#[test]
fn invalid_capture_budgets_are_rejected_before_provider_dispatch() {
    let runtime = Runtime::builder("invalid-capture-budget").configured_workers(1).build();
    let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    worker
        .register_task_provider(|_| panic!("invalid options must not dispatch requests"))
        .unwrap();
    let error = runtime
        .capture(CaptureOptions {
            limit_per_worker: 0,
            ..CaptureOptions::default()
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "runtime captures require 1..=4096 tasks, a response timeout up to five seconds, and a measurement interval up to one second"
    );
}
