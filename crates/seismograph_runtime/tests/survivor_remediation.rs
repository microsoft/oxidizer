// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contracts at the boundary between native task providers and inspection.

use seismograph::recorder::event::EventTimestamp;
use seismograph_runtime::snapshot::{CaptureOptions, Task, TaskActivityState, TaskCollection, TaskMetrics, TaskMetricsScope};
use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
use seismograph_runtime::{RuntimeMetadata, allocate_task_id, allocate_type_descriptor_id, register_runtime};

#[test]
fn providers_cannot_supply_poll_measurements_or_override_the_responding_worker() {
    let runtime = register_runtime(RuntimeMetadata::new("provider-measurements", 2));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let other = runtime.register_worker(WorkerMetadata::new(WorkerRole::Io));
    let other_id = other.id();
    let first = allocate_task_id();
    let second = allocate_task_id();
    let descriptor = allocate_type_descriptor_id();
    worker
        .handle()
        .register_task_provider(move |request| {
            let tasks = [second, first]
                .map(|id| {
                    let mut task = Task::new(id, descriptor, TaskActivityState::Ready);
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

    let captured = runtime.handle().capture(CaptureOptions::default()).unwrap();

    assert_eq!(captured.tasks.iter().map(|task| task.id).collect::<Vec<_>>(), [first, second]);
    for task in &captured.tasks {
        assert_eq!(task.last_worker_id, Some(worker.id()));
        assert_eq!((task.future_size_bytes, task.spawned_at), (None, None));
        assert_eq!(task.metrics, TaskMetrics::default());
        assert_eq!(task.metrics_scope, TaskMetricsScope::Uncollected);
    }
}

#[test]
fn retirement_through_registration_and_drop_rejects_later_inspection() {
    for explicit in [false, true] {
        let runtime = register_runtime(RuntimeMetadata::new("registration-retirement", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = runtime.handle();
        if explicit {
            runtime.stopped();
            assert_eq!(
                handle.capture(CaptureOptions::default()).unwrap_err().to_string(),
                "cannot inspect a stopped runtime"
            );
        }
        drop(runtime);
        assert_eq!(
            handle.capture(CaptureOptions::default()).unwrap_err().to_string(),
            "cannot inspect a stopped runtime"
        );
        assert_eq!(
            worker.handle().register_task_provider(drop).unwrap_err().to_string(),
            "cannot register a task provider on a stopped worker"
        );
    }
}

#[test]
fn invalid_capture_budgets_are_rejected_before_provider_dispatch() {
    let runtime = register_runtime(RuntimeMetadata::new("invalid-capture-budget", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    worker
        .handle()
        .register_task_provider(|_| panic!("invalid options must not dispatch requests"))
        .unwrap();
    let error = runtime
        .handle()
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
