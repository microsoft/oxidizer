// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This executable owns its process-wide recorder and runtime registry.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use seismograph::recorder::event::{BacktraceCapture, EventClass, EventKind, EventTimestamp, Record};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, TypeDescriptorId, WorkerId};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
use seismograph::snapshot::{DecodedSnapshot, EventBufferDisposition, SnapshotOptions};
use seismograph_runtime::snapshot::{
    Counters, CountersScope, RuntimeState, Task, TaskActivity, TaskActivityState, TaskCollection, TaskMetrics, TaskMetricsScope,
    WorkerState,
};
use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
use seismograph_runtime::{RuntimeMetadata, register_runtime};

fn capture(disposition: EventBufferDisposition) -> DecodedSnapshot {
    let bytes = seismograph::snapshot(SnapshotOptions {
        event_buffers: disposition,
    })
    .unwrap();
    seismograph::snapshot::decode(bytes.as_bytes()).unwrap()
}

fn runtime_source(snapshot: &DecodedSnapshot) -> seismograph_runtime::snapshot::Snapshot {
    let source = snapshot
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .unwrap();
    seismograph_runtime::snapshot::decode(&source.data).unwrap()
}

fn payload(
    runtime_id: RuntimeId,
    worker_id: Option<WorkerId>,
    subject_id: u64,
    related_id: u64,
    value_0: u64,
    value_1: u64,
) -> RuntimeEvent {
    RuntimeEvent {
        runtime_id,
        worker_id,
        subject_id,
        related_id,
        value_0,
        value_1,
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "One fixture owns the recording session and its ordered lifecycle assertions"
)]
fn gamma_round2_runtime_lifecycle_worker_transfers_and_terminal_payloads_are_exact() {
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(1024),
        ..Configuration::default()
    });
    let runtime = register_runtime(RuntimeMetadata::new("round2 payloads", 4).lifecycle_backtraces(BacktraceCapture::Never));
    let runtime_id = runtime.id();
    let first = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core).processor_index(17));
    let second = runtime.register_worker(WorkerMetadata::new(WorkerRole::Blocking));
    let worker = first.handle();
    let first_id = first.id();
    let second_id = second.id();
    let task_events = worker.task_events();
    let parent = task_events.spawned(TypeDescriptorId::from_raw(11).unwrap(), None, 0).unwrap();
    let child = task_events
        .spawned(TypeDescriptorId::from_raw(13).unwrap(), Some(parent), 0)
        .unwrap();
    let legacy = task_events
        .spawned(TypeDescriptorId::from_raw(17).unwrap(), Some(parent), 0)
        .unwrap();
    let sized = task_events
        .spawned(TypeDescriptorId::from_raw(19).unwrap(), Some(parent), 321)
        .unwrap();
    let tasks = Arc::new(Mutex::new(
        [(parent, 11, 0), (child, 13, 0), (legacy, 17, 0), (sized, 19, 321)]
            .map(|(id, descriptor, size)| Task {
                id,
                parent: (id != parent).then_some(parent),
                type_descriptor: TypeDescriptorId::from_raw(descriptor).unwrap(),
                future_size_bytes: Some(size),
                spawned_at: None,
                last_worker_id: None,
                metrics: TaskMetrics::default(),
                metrics_scope: TaskMetricsScope::Uncollected,
                activity: Some(TaskActivity {
                    observed_at: EventTimestamp::from_ticks(100),
                    state: TaskActivityState::Waiting,
                    ready_since: None,
                    poll_started_at: None,
                    poll_worker_id: None,
                    queued_since: None,
                }),
            })
            .to_vec(),
    ));
    worker
        .register_task_provider({
            let tasks = Arc::clone(&tasks);
            move |request| request.respond(tasks.lock().unwrap().clone(), TaskCollection::Complete)
        })
        .unwrap();
    second
        .handle()
        .register_task_provider(|request| request.respond(Vec::new(), TaskCollection::Complete))
        .unwrap();

    assert_initial_source(&capture(EventBufferDisposition::Retain), runtime_id, first_id, parent, sized);

    worker.parked();
    let parked = runtime_source(&capture(EventBufferDisposition::Retain));
    assert_eq!(parked.runtimes[0].workers[0].state, WorkerState::Parked);
    worker.unparked();
    for kind in [EventKind::TaskEnqueued, EventKind::TaskMaterialized] {
        seismograph::record(EventClass::RuntimeTask, || {
            Some(Record::runtime(
                EventTimestamp::from_ticks(100),
                kind,
                payload(runtime_id, Some(first_id), child.get(), 0, 0, 0),
                BacktraceCapture::Never,
            ))
        });
    }
    let poll = worker.task_poll_started(child);
    let polling = runtime_source(&capture(EventBufferDisposition::Retain));
    let task = polling.runtimes[0].tasks.iter().find(|value| value.id == child).unwrap();
    assert_eq!(task.metrics, TaskMetrics::default());
    assert_eq!(task.metrics_scope, TaskMetricsScope::Uncollected);
    assert_eq!(task.last_worker_id, Some(first_id));
    assert_eq!(polling.runtimes[0].workers[0].current_task, Some(child));
    worker.task_poll_finished(poll);
    {
        let mut tasks = tasks.lock().unwrap();
        let child = tasks.iter_mut().find(|task| task.id == child).unwrap();
        child.activity.as_mut().unwrap().state = TaskActivityState::Ready;
    }
    let ready = runtime_source(&capture(EventBufferDisposition::Retain));
    let ready = ready.runtimes[0]
        .tasks
        .iter()
        .find(|task| task.id == child)
        .unwrap()
        .activity
        .unwrap();
    assert_eq!(ready.state, TaskActivityState::Ready);
    assert_eq!((ready.ready_since, ready.queued_since), (None, None));
    let poll = worker.task_poll_started(child);
    worker.task_poll_finished(poll);
    let polled = runtime_source(&capture(EventBufferDisposition::Retain));
    let task = polled.runtimes[0].tasks.iter().find(|value| value.id == child).unwrap();
    assert_eq!(task.metrics, TaskMetrics::default());
    assert_eq!(task.metrics_scope, TaskMetricsScope::Uncollected);
    assert_eq!(polled.runtimes[0].workers[0].current_task, None);

    let transfer = worker.transfer_started(child, second_id);
    let transfer_id = capture(EventBufferDisposition::Retain)
        .events
        .events
        .iter()
        .find(|event| event.kind == EventKind::TransferStarted)
        .unwrap()
        .runtime()
        .unwrap()
        .subject_id;
    worker.instance_relocated(&transfer);
    worker.transfer_finished(transfer);
    task_events.completed(child);
    second.handle().task_events().canceled(parent);
    task_events.panicked(sized);
    task_events.completed(legacy);
    tasks.lock().unwrap().clear();
    runtime.stopping();
    runtime.stopping();
    runtime.stopped();
    runtime.stopped();
    drop(first);
    drop(second);

    let snapshot = capture(EventBufferDisposition::Stop);
    let actual = snapshot
        .events
        .events
        .iter()
        .map(|event| {
            assert!(event.call_stack.is_empty());
            let mut value = event.runtime().unwrap();
            if matches!(
                event.kind,
                EventKind::TaskPollStarted | EventKind::TaskPollFinished | EventKind::TransferFinished
            ) {
                value.value_0 = 0;
            }
            (event.kind, value)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        expected_events(runtime_id, [first_id, second_id], [parent, child, legacy, sized], transfer_id)
    );
    assert_terminal_source(&snapshot);
}

fn assert_initial_source(snapshot: &DecodedSnapshot, runtime_id: RuntimeId, worker: WorkerId, parent: TaskId, sized: TaskId) {
    let initial = runtime_source(snapshot);
    let initial = initial.runtimes.iter().find(|value| value.id == runtime_id).unwrap();
    assert_eq!(initial.state, RuntimeState::Running);
    assert_eq!(initial.configured_workers, 4);
    for task in &initial.tasks {
        assert_eq!(task.metrics, TaskMetrics::default());
        assert_eq!(task.metrics_scope, TaskMetricsScope::Uncollected);
        assert_eq!(task.spawned_at, None);
        assert_eq!(task.last_worker_id, Some(worker));
        assert_eq!(task.parent, (task.id != parent).then_some(parent));
        assert_eq!(task.future_size_bytes, Some(if task.id == sized { 321 } else { 0 }));
    }
    assert_eq!(initial.tasks.len(), 4);
    assert!(
        initial
            .workers
            .iter()
            .all(|value| value.thread_id.is_none() && value.current_task.is_none())
    );
}

fn expected_events(runtime_id: RuntimeId, workers: [WorkerId; 2], tasks: [TaskId; 4], transfer_id: u64) -> [(EventKind, RuntimeEvent); 26] {
    let [first_id, second_id] = workers;
    let [parent, child, legacy, sized] = tasks;
    let p = |worker, subject, related, first, second| payload(runtime_id, worker, subject, related, first, second);
    [
        (EventKind::RuntimeCreated, p(None, 0, 0, 4, 0)),
        (EventKind::WorkerStarted, p(Some(first_id), first_id.get(), 0, 1, 17)),
        (
            EventKind::WorkerStarted,
            p(Some(second_id), second_id.get(), 0, 2, u64::from(u32::MAX)),
        ),
        (EventKind::TaskSpawned, p(Some(first_id), parent.get(), 0, 11, 1)),
        (EventKind::TaskSpawned, p(Some(first_id), child.get(), parent.get(), 13, 1)),
        (EventKind::TaskSpawned, p(Some(first_id), legacy.get(), parent.get(), 17, 1)),
        (EventKind::TaskSpawned, p(Some(first_id), sized.get(), parent.get(), 19, 322)),
        (EventKind::WorkerParked, p(Some(first_id), first_id.get(), 0, 0, 0)),
        (EventKind::WorkerUnparked, p(Some(first_id), first_id.get(), 0, 0, 0)),
        (EventKind::TaskEnqueued, p(Some(first_id), child.get(), 0, 0, 0)),
        (EventKind::TaskMaterialized, p(Some(first_id), child.get(), 0, 0, 0)),
        (EventKind::TaskPollStarted, p(Some(first_id), child.get(), 0, 0, 0)),
        (EventKind::TaskPollFinished, p(Some(first_id), child.get(), 0, 0, 0)),
        (EventKind::TaskPollStarted, p(Some(first_id), child.get(), 0, 0, 0)),
        (EventKind::TaskPollFinished, p(Some(first_id), child.get(), 0, 0, 0)),
        (
            EventKind::TransferStarted,
            p(Some(first_id), transfer_id, child.get(), second_id.get(), first_id.get()),
        ),
        (
            EventKind::InstanceRelocated,
            p(Some(first_id), transfer_id, child.get(), first_id.get(), second_id.get()),
        ),
        (
            EventKind::TransferFinished,
            p(Some(first_id), transfer_id, child.get(), 0, second_id.get()),
        ),
        (EventKind::TaskCompleted, p(Some(first_id), child.get(), 0, 0, 0)),
        (EventKind::TaskCanceled, p(Some(second_id), parent.get(), 0, 0, 0)),
        (EventKind::TaskPanicked, p(Some(first_id), sized.get(), 0, 0, 0)),
        (EventKind::TaskCompleted, p(Some(first_id), legacy.get(), 0, 0, 0)),
        (EventKind::RuntimeStopping, p(None, 0, 0, 0, 0)),
        (EventKind::WorkerStopped, p(Some(first_id), first_id.get(), 0, 0, 0)),
        (EventKind::WorkerStopped, p(Some(second_id), second_id.get(), 0, 0, 0)),
        (EventKind::RuntimeStopped, p(None, 0, 0, 0, 0)),
    ]
}

fn assert_terminal_source(snapshot: &DecodedSnapshot) {
    let final_source = runtime_source(snapshot);
    let final_runtime = &final_source.runtimes[0];
    assert_eq!(final_runtime.counters_scope, CountersScope::Uncollected);
    assert_eq!(final_runtime.counters, Counters::default());
    assert!(final_runtime.tasks.is_empty());
    assert_eq!(final_runtime.state, RuntimeState::Stopped);
    assert!(
        final_runtime
            .workers
            .iter()
            .all(|value| value.state == WorkerState::Stopped && value.current_task.is_none())
    );
}
