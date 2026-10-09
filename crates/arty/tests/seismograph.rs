// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Public runtime and task lifecycle contracts exposed through Seismograph snapshots.

testing_aids::init_tracing!();

mod support;

use std::sync::mpsc;
use std::task::Poll;

use arty::runtime::{Runtime, WorkersPolicy};
use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, RecordingPolicy};
use seismograph::snapshot::SnapshotOptions;
use seismograph_runtime::snapshot::{RuntimeState, source};
use support::JoinHandleExt as _;
use thread_aware::Unaware;

struct RecorderReset;

impl Drop for RecorderReset {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

#[test]
fn public_spawn_paths_report_runtime_worker_task_and_poll_lifecycle() {
    let _reset = RecorderReset;
    seismograph::recorder(Configuration {
        runtime_tasks: RecordingPolicy::all(false),
        ..Configuration::default()
    });

    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 17u32 }).join().unwrap(), 17);
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
                .unwrap();
        })
        .join()
        .unwrap();
    assert!(
        runtime
            .scheduler()
            .spawn_anywhere::<(), _, ()>((), |_, ()| async { panic!("seismograph panic contract") })
            .join()
            .unwrap_err()
            .is_panic()
    );
    let (started, materialized) = mpsc::channel();
    let canceled = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started), |_, Unaware(started)| async move {
            started.send(()).unwrap();
            std::future::pending::<()>().await;
        });
    materialized.recv_timeout(testing_aids::TEST_TIMEOUT).unwrap();
    runtime.stop().unwrap();
    assert!(canceled.join().unwrap_err().is_shutdown());

    let other = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    other.scheduler().spawn_anywhere((), |_, ()| async {}).join().unwrap();
    other.stop().unwrap();

    let snapshot = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let runtime_source = decoded.sources.iter().find(|entry| entry.id == source::ID).unwrap();
    let runtime_snapshot = seismograph_runtime::snapshot::decode(&runtime_source.data).unwrap();
    let arty_runtimes: Vec<_> = runtime_snapshot.runtimes.iter().filter(|entry| entry.name == "arty").collect();

    assert_eq!(arty_runtimes.len(), 2);
    assert_ne!(arty_runtimes[0].id, arty_runtimes[1].id);
    assert!(arty_runtimes.iter().all(|entry| entry.state == RuntimeState::Stopped));
    assert!(
        arty_runtimes.iter().all(|entry| {
            entry.workers.len() == 1 && entry.workers[0].thread_id.is_some() && entry.workers[0].processor_index == Some(0)
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
