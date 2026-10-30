// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native-storage collection is independent of event recording.

use std::sync::{Arc, Mutex, mpsc};

use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, Policy, without_recording};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};
use seismograph_runtime::snapshot::{CaptureOptions, CountersScope, Task, TaskActivityState, TaskCollection, TaskMetricsScope};
use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
use seismograph_runtime::{RuntimeMetadata, allocate_task_id, allocate_type_descriptor_id, register_runtime};

static RECORDING: Mutex<()> = Mutex::new(());

#[test]
fn current_native_state_is_available_without_event_recording() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("native-off", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let handle = worker.handle();
    let task = allocate_task_id();
    let descriptor = allocate_type_descriptor_id();
    handle
        .register_task_provider(move |request| {
            request.respond(
                vec![Task::new(task, descriptor, TaskActivityState::Ready)],
                TaskCollection::Complete,
            );
        })
        .unwrap();

    let captured = runtime.handle().capture(CaptureOptions::default()).unwrap();

    assert_eq!(
        (
            captured.counters_scope,
            captured.workers[0].task_collection,
            captured.tasks[0].id,
            captured.tasks[0].activity.unwrap().state,
            captured.tasks[0].activity.unwrap().ready_since,
            captured.tasks[0].spawned_at,
            captured.tasks[0].metrics_scope,
            handle.is_observed(),
        ),
        (
            CountersScope::Uncollected,
            TaskCollection::Complete,
            task,
            TaskActivityState::Ready,
            None,
            None,
            TaskMetricsScope::Uncollected,
            false,
        )
    );
}

#[test]
fn stopping_events_does_not_freeze_native_task_state() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = register_runtime(RuntimeMetadata::new("stop-is-not-runtime-stop", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let state = Arc::new(Mutex::new(TaskActivityState::Ready));
    let provider_state = Arc::clone(&state);
    let task = allocate_task_id();
    let descriptor = allocate_type_descriptor_id();
    worker
        .handle()
        .register_task_provider(move |request| {
            let state = *provider_state.lock().unwrap();
            request.respond(vec![Task::new(task, descriptor, state)], TaskCollection::Complete);
        })
        .unwrap();
    seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    *state.lock().unwrap() = TaskActivityState::Waiting;

    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
    let source = snapshot
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .unwrap();
    let decoded = seismograph_runtime::snapshot::decode(&source.data).unwrap();
    let observed = decoded.runtimes.iter().find(|item| item.id == runtime.id()).unwrap();

    assert_eq!(observed.tasks[0].activity.unwrap().state, TaskActivityState::Waiting);
}

#[test]
fn concurrent_requests_report_busy_and_release_observation_demand() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("bounded-requests", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let handle = worker.handle();
    let (sender, receiver) = mpsc::channel();
    handle.register_task_provider(move |request| sender.send(request).unwrap()).unwrap();
    let caller = runtime.handle();
    let capture = std::thread::spawn(move || caller.capture(CaptureOptions::default()).unwrap());
    let request = receiver.recv().unwrap();
    assert!(handle.is_observed());

    let busy = runtime.handle().capture(CaptureOptions::default()).unwrap();
    request.respond(Vec::new(), TaskCollection::Complete);
    let answered = capture.join().unwrap();

    assert_eq!(
        (
            busy.workers[0].task_collection,
            answered.workers[0].task_collection,
            handle.is_observed()
        ),
        (TaskCollection::Busy, TaskCollection::Complete, false)
    );
}

#[test]
fn dropped_providers_and_missing_providers_are_not_empty_complete_lists() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("availability", 2));
    let absent = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let dropped = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    dropped.handle().register_task_provider(drop).unwrap();

    let captured = runtime.handle().capture(CaptureOptions::default()).unwrap();

    assert_eq!(
        captured
            .workers
            .iter()
            .map(|worker| (worker.id, worker.task_collection))
            .collect::<Vec<_>>(),
        [(absent.id(), TaskCollection::Unavailable), (dropped.id(), TaskCollection::Failed)]
    );
}

#[test]
fn collection_truncates_an_oversized_provider_response() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("truncated", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    worker
        .handle()
        .register_task_provider(|request| {
            let descriptor = allocate_type_descriptor_id();
            request.respond(
                vec![Task::new(allocate_task_id(), descriptor, TaskActivityState::Waiting); 2],
                TaskCollection::Complete,
            );
        })
        .unwrap();

    let captured = runtime
        .handle()
        .capture(CaptureOptions {
            limit_per_worker: 1,
            ..CaptureOptions::default()
        })
        .unwrap();

    assert_eq!(
        (captured.tasks.len(), captured.workers[0].task_collection),
        (1, TaskCollection::Truncated)
    );
}

#[test]
fn unrelated_policies_and_suppression_do_not_activate_poll_instrumentation() {
    let _lock = RECORDING.lock().unwrap();
    let runtime = register_runtime(RuntimeMetadata::new("demand", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let handle = worker.handle();
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        ..Configuration::default()
    });
    let unrelated = handle.is_observed();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let active = handle.is_observed();
    let suppressed = without_recording(|| handle.is_observed());
    seismograph::recorder(Configuration::default());

    assert_eq!((unrelated, active, suppressed, handle.is_observed()), (false, true, false, false));
}

#[test]
fn abandoning_an_observed_poll_clears_its_worker_slot() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("abandoned-poll", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let id = allocate_task_id();
    let poll = worker.handle().task_poll_started(id);
    let during = runtime.handle().capture(CaptureOptions::default()).unwrap();

    drop(poll);
    let after = runtime.handle().capture(CaptureOptions::default()).unwrap();

    assert_eq!((during.workers[0].current_task, after.workers[0].current_task), (Some(id), None));
}

#[test]
fn recording_retains_task_creation_and_real_outcomes_without_a_task_registry() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("recorded-task-lifecycle", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let events = worker.handle().task_events();
    let descriptor = allocate_type_descriptor_id();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(true),
        ..Configuration::default()
    });

    let completed = events.spawned(descriptor, None, 0).unwrap();
    events.completed(completed);
    let panicked = events.spawned(descriptor, Some(completed), 64).unwrap();
    events.panicked(panicked);
    let canceled = events.spawned(descriptor, None, 4096).unwrap();
    events.canceled(canceled);
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
    seismograph::recorder(Configuration::default());
    let events = snapshot
        .events
        .events
        .iter()
        .filter(|event| event.runtime().is_some_and(|event| event.runtime_id == runtime.id()))
        .collect::<Vec<_>>();

    assert_eq!(
        events
            .iter()
            .map(|event| {
                let payload = event.runtime().unwrap();
                (event.kind, payload.subject_id, payload.related_id, payload.value_0, payload.value_1)
            })
            .collect::<Vec<_>>(),
        [
            (EventKind::TaskSpawned, completed.get(), 0, descriptor.get(), 1),
            (EventKind::TaskCompleted, completed.get(), 0, 0, 0),
            (EventKind::TaskSpawned, panicked.get(), completed.get(), descriptor.get(), 65),
            (EventKind::TaskPanicked, panicked.get(), 0, 0, 0),
            (EventKind::TaskSpawned, canceled.get(), 0, descriptor.get(), 4097),
            (EventKind::TaskCanceled, canceled.get(), 0, 0, 0),
        ]
    );
    #[cfg(all(not(miri), any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    assert!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::TaskSpawned)
            .all(|event| !event.call_stack.is_empty())
    );
}

#[test]
fn disabled_task_event_context_does_not_initialize_event_buffers() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("disabled-task-events", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let context = worker.handle().task_events();
    let id = allocate_task_id();
    let descriptor = allocate_type_descriptor_id();

    let created = context.spawned(descriptor, None, 64);
    context.completed(id);
    context.panicked(id);
    context.canceled(id);
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();

    assert_eq!((created, snapshot.events.events.len(), snapshot.events.threads.len()), (None, 0, 0));
}
