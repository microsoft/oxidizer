// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Public runtime and task lifecycle contracts exposed through Seismograph snapshots.

testing_aids::init_tracing!();

mod support;

use std::sync::mpsc;
use std::task::Poll;

use arty::runtime::{Runtime, WorkersPolicy};
use arty::task::Scheduler;
use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, RecordingPolicy};
use seismograph::snapshot::{DecodedSnapshot, SnapshotOptions};
use seismograph_runtime::snapshot::{Runtime as RuntimeSnapshot, RuntimeState, source};
use support::JoinHandleExt as _;
use thread_aware::Unaware;

struct RecorderReset;

impl Drop for RecorderReset {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

#[expect(clippy::panic, reason = "this scenario verifies Seismograph panic classification")]
fn exercise_primary_runtime() -> Scheduler {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the primary runtime requires one available worker");
    let (value, retained_scheduler) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { (17u32, cx.scheduler().clone()) })
        .join()
        .expect("the first runtime-wide task completes");
    assert_eq!(value, 17);
    runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move {
            cx.scheduler()
                .spawn(async |_| {
                    let mut first_poll = true;
                    std::future::poll_fn(move |poll| {
                        if std::mem::replace(&mut first_poll, false) {
                            poll.waker().wake_by_ref();
                            Poll::Pending
                        } else {
                            Poll::Ready(())
                        }
                    })
                    .await;
                })
                .await
                .expect("the direct worker-local child completes");
        })
        .join()
        .expect("the parent of the direct worker-local child completes");
    assert!(
        runtime
            .scheduler()
            .spawn_anywhere::<(), _, ()>((), |_, ()| async { std::panic::panic_any("seismograph panic contract") })
            .join()
            .expect_err("the intentional panic reaches the join handle")
            .is_panic()
    );
    let (started, materialized) = mpsc::channel();
    let canceled = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started), |_, Unaware(started)| async move {
            started.send(()).expect("the test retains the materialization receiver");
            std::future::pending::<()>().await;
        });
    materialized
        .recv_timeout(testing_aids::TEST_TIMEOUT)
        .expect("the pending task materializes before shutdown");
    runtime.stop().expect("the primary runtime stops cleanly");
    assert!(canceled.join().expect_err("shutdown cancels the pending task").is_shutdown());
    retained_scheduler
}

fn exercise_stopped_runtime() {
    let other = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the secondary runtime requires one available worker");
    other
        .scheduler()
        .spawn_anywhere((), |_, ()| async {})
        .join()
        .expect("the secondary runtime task completes");
    other.stop().expect("the secondary runtime stops cleanly");
}

fn exercise_owner_drop_runtime() -> Scheduler {
    let owner_dropped = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the owner-drop runtime requires one available worker");
    let owner_drop_scheduler = owner_dropped
        .scheduler()
        .block_on(async |cx| cx.scheduler().clone())
        .expect("the runtime returns its worker-local scheduler");
    owner_drop_scheduler
        .spawn(async move |_| drop(owner_dropped))
        .join()
        .expect("dropping the owner on its worker does not unwind");
    owner_drop_scheduler
}

fn assert_primary_events(decoded: &DecodedSnapshot, primary: &RuntimeSnapshot) {
    let events: Vec<_> = decoded
        .events
        .events
        .iter()
        .filter_map(|event| {
            let payload = event.runtime()?;
            (payload.runtime_id == primary.id).then_some((event.kind, payload.worker_id))
        })
        .collect();
    let count = |kind| events.iter().filter(|(event_kind, _)| *event_kind == kind).count();
    assert_eq!(count(EventKind::RuntimeStopping), 1);
    assert_eq!(count(EventKind::RuntimeStopped), 1);
    assert_eq!(count(EventKind::WorkerStarted), 1);
    assert_eq!(count(EventKind::WorkerStopped), 1);
    assert_eq!(count(EventKind::TaskEnqueued), 5);
    assert_eq!(count(EventKind::TaskMaterialized), 5);
    assert_eq!(count(EventKind::TaskCompleted), 3);
    assert_eq!(count(EventKind::TaskPanicked), 1);
    assert_eq!(count(EventKind::TaskCanceled), 1);
    assert_eq!(count(EventKind::TaskPollStarted), count(EventKind::TaskPollFinished));
    assert!(
        events
            .iter()
            .filter(|(kind, _)| matches!(
                kind,
                EventKind::TaskMaterialized
                    | EventKind::TaskPollStarted
                    | EventKind::TaskPollFinished
                    | EventKind::TaskCompleted
                    | EventKind::TaskPanicked
                    | EventKind::TaskCanceled
            ))
            .all(|(_, worker)| *worker == Some(primary.workers[0].id))
    );
}

#[test]
fn public_spawn_paths_report_runtime_worker_task_and_poll_lifecycle() {
    let _reset = RecorderReset;
    seismograph::recorder(Configuration {
        runtime_tasks: RecordingPolicy::all(false),
        ..Configuration::default()
    });
    let retained_scheduler = exercise_primary_runtime();
    exercise_stopped_runtime();
    let owner_drop_scheduler = exercise_owner_drop_runtime();
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).expect("the Seismograph snapshot encodes");
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).expect("the Seismograph snapshot decodes");
    let runtime_source = decoded
        .sources
        .iter()
        .find(|entry| entry.id == source::ID)
        .expect("the runtime source is registered");
    let runtime_snapshot = seismograph_runtime::snapshot::decode(&runtime_source.data).expect("the runtime source payload decodes");
    let arty_runtimes: Vec<_> = runtime_snapshot.runtimes.iter().filter(|entry| entry.name == "arty").collect();

    assert_eq!(arty_runtimes.len(), 3);
    assert_ne!(arty_runtimes[0].id, arty_runtimes[1].id);
    assert_eq!(arty_runtimes.iter().filter(|entry| entry.state == RuntimeState::Stopped).count(), 2);
    assert_eq!(
        arty_runtimes.iter().filter(|entry| entry.state == RuntimeState::Stopping).count(),
        1
    );
    assert!(
        arty_runtimes.iter().all(|entry| {
            entry.workers.len() == 1 && entry.workers[0].thread_id.is_some() && entry.workers[0].processor_index.is_some()
        })
    );

    let primary = arty_runtimes
        .iter()
        .find(|entry| entry.counters.spawned_tasks == 5)
        .expect("the primary runtime records every public async spawn path");
    assert_eq!(
        (
            primary.counters.live_tasks,
            primary.counters.completed_tasks,
            primary.counters.panicked_tasks,
            primary.counters.canceled_tasks,
        ),
        (0, 3, 1, 1)
    );
    assert!(primary.counters.poll_count >= 6);
    assert_primary_events(&decoded, primary);
    drop(retained_scheduler);
    drop(owner_drop_scheduler);
}
