// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Recording-only task state remains gated independently of the global generation.

use seismograph::recorder::runtime::TypeDescriptorId;
use seismograph::recorder::{Configuration, Policy, generation, without_recording};
use seismograph::snapshot::SnapshotOptions;
use seismograph_runtime::snapshot::TaskActivityState;
use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
use seismograph_runtime::{RuntimeMetadata, register_runtime};

#[test]
fn active_generations_do_not_enable_disabled_or_suppressed_runtime_tracking() {
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        ..Default::default()
    });
    assert!(generation().is_some());
    let runtime = register_runtime(RuntimeMetadata::new("class-policy", 1));
    let handle = runtime.handle();
    let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core)).handle();
    let capture = |task_id| {
        let encoded = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(encoded.as_bytes()).unwrap();
        let source = decoded
            .sources
            .iter()
            .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
            .unwrap();
        let source = seismograph_runtime::snapshot::decode(&source.data).unwrap();
        source
            .runtimes
            .into_iter()
            .find(|entry| entry.id == runtime.id())
            .unwrap()
            .tasks
            .into_iter()
            .find(|task| task.id == task_id)
            .unwrap()
    };
    let disabled = handle.register_task(TypeDescriptorId::from_raw(1).unwrap(), None);
    disabled.woken();
    let poll = disabled.poll_started(&worker);
    disabled.poll_finished(&worker, poll);
    let disabled = capture(disabled.id());

    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        runtime_tasks: Policy::all(false),
        ..Default::default()
    });
    let suppressed = without_recording(|| {
        let task = handle.register_task(TypeDescriptorId::from_raw(2).unwrap(), None);
        task.woken();
        let poll = task.poll_started(&worker);
        task.poll_finished(&worker, poll);
        task
    });
    let suppressed = capture(suppressed.id());
    seismograph::recorder(Configuration::default());
    let observed = |task: seismograph_runtime::snapshot::Task| {
        let activity = task.activity.unwrap();
        (
            activity.state,
            activity.ready_since,
            activity.poll_started_at,
            activity.poll_worker_id,
            activity.queued_since,
            task.metrics.poll_count,
        )
    };

    assert_eq!(
        (observed(disabled), observed(suppressed)),
        (
            (TaskActivityState::Unknown, None, None, None, None, 1),
            (TaskActivityState::Unknown, None, None, None, None, 1),
        )
    );
}
