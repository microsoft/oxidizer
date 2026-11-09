// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native-storage collection is independent of event recording.

use std::sync::{Arc, Mutex};

use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, Policy, without_recording};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};
use seismograph_runtime::snapshot::{CaptureOptions, CountersScope, Task, TaskActivityState, TaskCollection, TaskMetricsScope};
use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
use seismograph_runtime::{RuntimeMetadata, allocate_task_id, allocate_type_descriptor_id, register_runtime};

static RECORDING: Mutex<()> = Mutex::new(());

#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper follows the same unwrap policy as its test callers"
)]
fn source_snapshot() -> seismograph_runtime::snapshot::Snapshot {
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
    let source = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .unwrap();
    seismograph_runtime::snapshot::decode(&source.data).unwrap()
}

#[test]
fn all_native_constructor_states_survive_process_snapshot_encoding() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("constructor-states", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let descriptor = allocate_type_descriptor_id();
    let states = [
        TaskActivityState::Unknown,
        TaskActivityState::Waiting,
        TaskActivityState::Ready,
        TaskActivityState::Running,
    ];
    worker
        .handle()
        .register_task_provider(move |request| {
            request.respond(
                states.map(|state| Task::new(allocate_task_id(), descriptor, state)).to_vec(),
                TaskCollection::Complete,
            );
        })
        .unwrap();

    let snapshot = source_snapshot();
    let observed = snapshot.runtimes.iter().find(|item| item.id == runtime.id()).unwrap();

    assert_eq!(
        observed
            .tasks
            .iter()
            .map(|task| {
                let activity = task.activity.unwrap();
                (
                    activity.state,
                    activity.poll_started_at,
                    activity.ready_since,
                    activity.queued_since,
                )
            })
            .collect::<Vec<_>>(),
        states.map(|state| (state, None, None, None))
    );
}

#[test]
fn worker_metadata_transitions_and_runtime_retirement_remain_visible() {
    use seismograph_runtime::snapshot::{RuntimeState, WorkerState};
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("worker-lifecycle", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core).processor_index(7));
    let handle = worker.handle();
    worker.attach_current_thread();
    handle.parked();
    let parked = runtime.handle().capture(CaptureOptions::default()).unwrap();
    handle.unparked();
    let running = runtime.handle().capture(CaptureOptions::default()).unwrap();
    runtime.stopping();
    runtime.stopped();
    let snapshot = source_snapshot();
    let stopped = snapshot.runtimes.iter().find(|item| item.id == runtime.id()).unwrap();

    assert_eq!(
        (
            parked.workers[0].processor_index,
            parked.workers[0].thread_id,
            parked.workers[0].state,
            running.workers[0].state,
            stopped.state,
            stopped.retired_at.is_some(),
            stopped.workers[0].state
        ),
        (
            Some(7),
            Some(seismograph::recorder::thread_id()),
            WorkerState::Parked,
            WorkerState::Running,
            RuntimeState::Stopped,
            true,
            WorkerState::Stopped
        )
    );
}

#[test]
fn metadata_backtrace_override_controls_lifecycle_and_spawn_events() {
    use seismograph::recorder::event::BacktraceCapture;
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(true),
        ..Configuration::default()
    });
    let runtime = register_runtime(RuntimeMetadata::new("no-lifecycle-stacks", 1).lifecycle_backtraces(BacktraceCapture::Never));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));

    worker
        .handle()
        .task_events()
        .spawned(allocate_type_descriptor_id(), None, 0)
        .unwrap();
    let captured = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let captured = seismograph::snapshot::decode(captured.as_bytes()).unwrap();
    seismograph::recorder(Configuration::default());
    let events = captured
        .events
        .events
        .iter()
        .filter(|event| event.runtime().is_some_and(|event| event.runtime_id == runtime.id()))
        .map(|event| (event.kind, event.call_stack.is_empty()))
        .collect::<Vec<_>>();

    assert_eq!(
        events,
        [
            (EventKind::RuntimeCreated, true),
            (EventKind::WorkerStarted, true),
            (EventKind::TaskSpawned, true)
        ]
    );
}

#[test]
fn finishing_an_observed_poll_records_the_pair_and_releases_the_worker_slot() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = register_runtime(RuntimeMetadata::new("finished-poll", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let handle = worker.handle();
    let task = allocate_task_id();

    let poll = handle.task_poll_started(task);
    let during = runtime.handle().capture(CaptureOptions::default()).unwrap();
    handle.task_poll_finished(poll);
    let after = runtime.handle().capture(CaptureOptions::default()).unwrap();
    let captured = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let captured = seismograph::snapshot::decode(captured.as_bytes()).unwrap();
    seismograph::recorder(Configuration::default());
    let events = captured
        .events
        .events
        .iter()
        .filter(|event| {
            event
                .runtime()
                .is_some_and(|event| event.runtime_id == runtime.id() && event.subject_id == task.get())
                && matches!(event.kind, EventKind::TaskPollStarted | EventKind::TaskPollFinished)
        })
        .map(|event| event.kind)
        .collect::<Vec<_>>();

    assert_eq!(
        (during.workers[0].current_task, after.workers[0].current_task, events),
        (Some(task), None, vec![EventKind::TaskPollStarted, EventKind::TaskPollFinished])
    );
}

#[test]
fn runtime_and_transfer_events_preserve_one_time_lifecycle_and_payloads() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let runtime = register_runtime(RuntimeMetadata::new("recorded-lifecycle", 2));
    let first = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let second = runtime.register_worker(WorkerMetadata::new(WorkerRole::Io));
    let id = allocate_task_id();
    let transfer = first.handle().transfer_started(id, second.id());
    first.handle().instance_relocated(&transfer);
    first.handle().transfer_finished(transfer);
    runtime.stopping();
    runtime.stopping();
    runtime.stopped();
    runtime.stopped();
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let captured = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
    seismograph::recorder(Configuration::default());
    let events = captured
        .events
        .events
        .iter()
        .filter(|event| event.runtime().is_some_and(|payload| payload.runtime_id == runtime.id()))
        .collect::<Vec<_>>();

    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            EventKind::RuntimeCreated,
            EventKind::WorkerStarted,
            EventKind::WorkerStarted,
            EventKind::TransferStarted,
            EventKind::InstanceRelocated,
            EventKind::TransferFinished,
            EventKind::RuntimeStopping,
            EventKind::WorkerStopped,
            EventKind::WorkerStopped,
            EventKind::RuntimeStopped
        ]
    );
    let started = events[3].runtime().unwrap();
    let relocated = events[4].runtime().unwrap();
    let finished = events[5].runtime().unwrap();
    assert_eq!(
        (
            started.subject_id,
            started.related_id,
            started.value_0,
            started.value_1,
            relocated.subject_id,
            relocated.related_id,
            relocated.value_0,
            relocated.value_1,
            finished.subject_id,
            finished.related_id,
            finished.value_1
        ),
        (
            started.subject_id,
            id.get(),
            second.id().get(),
            first.id().get(),
            started.subject_id,
            id.get(),
            first.id().get(),
            second.id().get(),
            started.subject_id,
            id.get(),
            second.id().get()
        )
    );
}

#[test]
fn rejected_worker_registration_does_not_poison_later_source_captures() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("registration-after-stop", 1));
    runtime.stopped();

    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runtime.register_worker(WorkerMetadata::new(WorkerRole::Core))
    }));
    let source = source_snapshot();

    rejected.unwrap_err();
    assert!(
        source
            .runtimes
            .iter()
            .find(|item| item.id == runtime.id())
            .unwrap()
            .workers
            .is_empty()
    );
}

#[test]
fn provider_drop_panic_cannot_leave_later_workers_running() {
    use seismograph_runtime::snapshot::WorkerState;
    struct BadDrop;
    impl Drop for BadDrop {
        fn drop(&mut self) {
            panic!("provider capture destructor");
        }
    }
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("retire-before-drop", 2));
    let first = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let second = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let bad = BadDrop;
    first
        .handle()
        .register_task_provider(move |request| {
            let _keep = &bad;
            request.respond(Vec::new(), TaskCollection::Complete);
        })
        .unwrap();
    second
        .handle()
        .register_task_provider(|_| panic!("retired provider must never run"))
        .unwrap();

    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.stopped()));
    let source = source_snapshot();
    let retired = source.runtimes.iter().find(|item| item.id == runtime.id()).unwrap();

    stopped.unwrap_err();
    assert_eq!(
        retired
            .workers
            .iter()
            .map(|worker| (worker.state, worker.task_collection))
            .collect::<Vec<_>>(),
        [(WorkerState::Stopped, TaskCollection::Stopped); 2]
    );
    runtime.handle().capture(CaptureOptions::default()).unwrap_err();
    second.handle().register_task_provider(drop).unwrap_err();
}

#[test]
fn capture_rejects_each_invalid_budget_without_dispatching() {
    use std::time::Duration;
    let _lock = RECORDING.lock().unwrap();
    let runtime = register_runtime(RuntimeMetadata::new("invalid-capture", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    worker
        .handle()
        .register_task_provider(|_| panic!("invalid capture dispatched"))
        .unwrap();

    let invalid = [
        CaptureOptions {
            limit_per_worker: 0,
            ..CaptureOptions::default()
        },
        CaptureOptions {
            limit_per_worker: 4097,
            ..CaptureOptions::default()
        },
        CaptureOptions {
            timeout: Duration::ZERO,
            ..CaptureOptions::default()
        },
        CaptureOptions {
            timeout: Duration::from_secs(5) + Duration::from_nanos(1),
            ..CaptureOptions::default()
        },
        CaptureOptions {
            measure_for: Duration::from_secs(1) + Duration::from_nanos(1),
            ..CaptureOptions::default()
        },
    ]
    .map(|options| runtime.handle().capture(options).is_err());

    assert_eq!(invalid, [true; 5]);
}

#[test]
fn event_and_capture_values_preserve_copy_and_unwind_contracts() {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    fn value<T: Send + Sync + UnwindSafe + RefUnwindSafe>() {}
    fn copied<T: Copy>() {}
    value::<seismograph_runtime::task::EventContext>();
    copied::<seismograph_runtime::task::EventContext>();
    value::<seismograph_runtime::task::TaskSpawn>();
    copied::<seismograph_runtime::task::TaskSpawn>();
    value::<CaptureOptions>();
    value::<TaskCollection>();
    value::<CountersScope>();
    value::<TaskMetricsScope>();
    static_assertions::assert_impl_all!(seismograph_runtime::task::TaskRequest: Send, Sync, UnwindSafe, RefUnwindSafe);
}

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
    let task = allocate_task_id();
    let descriptor = allocate_type_descriptor_id();
    worker
        .handle()
        .register_task_provider({
            let state = Arc::clone(&state);
            move |request| {
                let state = *state.lock().unwrap();
                request.respond(vec![Task::new(task, descriptor, state)], TaskCollection::Complete);
            }
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
fn a_response_exactly_at_the_limit_remains_complete() {
    let _lock = RECORDING.lock().unwrap();
    let runtime = register_runtime(RuntimeMetadata::new("exact-limit", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    worker
        .handle()
        .register_task_provider(|request| {
            request.respond(
                vec![Task::new(
                    allocate_task_id(),
                    allocate_type_descriptor_id(),
                    TaskActivityState::Waiting,
                )],
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
        (1, TaskCollection::Complete)
    );
}

#[test]
fn dropping_a_worker_retires_its_provider_without_stopping_the_runtime() {
    use seismograph_runtime::snapshot::{RuntimeState, WorkerState};
    let _lock = RECORDING.lock().unwrap();
    let runtime = register_runtime(RuntimeMetadata::new("worker-retirement", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let handle = worker.handle();
    handle
        .register_task_provider(|_| panic!("retired provider must never run"))
        .unwrap();

    drop(worker);
    let captured = runtime.handle().capture(CaptureOptions::default()).unwrap();

    assert_eq!(
        (
            captured.state,
            captured.workers[0].state,
            captured.workers[0].task_collection,
            handle.register_task_provider(drop).is_err()
        ),
        (RuntimeState::Running, WorkerState::Stopped, TaskCollection::Stopped, true)
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
    // Disabling recording preserves earlier tests' buffers. Release them before
    // checking that this disabled path creates none.
    drop(
        seismograph::snapshot(SnapshotOptions {
            event_buffers: EventBufferDisposition::Stop,
        })
        .unwrap(),
    );
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

#[test]
fn lazy_spawn_metadata_matches_the_recorded_event() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("lazy-spawn-metadata", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let context = worker.handle().task_events();
    let descriptor = allocate_type_descriptor_id();
    let parent = allocate_task_id();
    let calls = std::cell::Cell::new(0);
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });

    let spawned = context
        .spawned_with(
            || {
                calls.set(calls.get() + 1);
                descriptor
            },
            Some(parent),
            64,
        )
        .unwrap();
    let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
    seismograph::recorder(Configuration::default());
    let event = snapshot
        .events
        .events
        .iter()
        .find(|event| {
            event.kind == EventKind::TaskSpawned
                && event
                    .runtime()
                    .is_some_and(|payload| payload.runtime_id == runtime.id() && payload.subject_id == spawned.id.get())
        })
        .unwrap();
    let payload = event.runtime().unwrap();

    assert_eq!(
        (
            calls.get(),
            spawned.descriptor,
            spawned.timestamp,
            payload.related_id,
            payload.value_0,
            payload.value_1
        ),
        (1, descriptor, event.timestamp, parent.get(), descriptor.get(), 65)
    );
}

#[test]
fn disabled_and_suppressed_spawns_do_not_build_type_metadata() {
    let _lock = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let runtime = register_runtime(RuntimeMetadata::new("lazy-disabled-spawn", 1));
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
    let context = worker.handle().task_events();
    let descriptor = || panic!("disabled or suppressed recording must not build task type metadata");

    let disabled = context.spawned_with(descriptor, None, 0);
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let suppressed = without_recording(|| context.spawned_with(descriptor, None, 0));
    seismograph::recorder(Configuration::default());

    assert_eq!((disabled, suppressed), (None, None));
}
