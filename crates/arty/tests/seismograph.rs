// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Public runtime and task lifecycle contracts exposed through Seismograph snapshots.

testing_aids::init_tracing!();

mod support;

use std::collections::HashSet;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, Wake, Waker};

use arty::runtime::{Runtime, RuntimeOperations, WorkersPolicy};
use arty::task::Scheduler;
use seismograph::recorder::event::EventKind;
use seismograph::recorder::runtime::RuntimeId;
use seismograph::recorder::{Configuration, RecordingPolicy};
use seismograph::snapshot::{DecodedSnapshot, SnapshotOptions};
use seismograph_runtime::snapshot::{Runtime as RuntimeSnapshot, RuntimeState, Snapshot as RuntimeSourceSnapshot, source};
use support::JoinHandleExt as _;
use thread_aware::Unaware;

struct RecorderReset;

impl Drop for RecorderReset {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

struct BlockingWake {
    notified: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Wake for BlockingWake {
    fn wake(self: Arc<Self>) {
        self.notified.send(()).expect("the test retains the wake notification receiver");
        self.release
            .lock()
            .expect("the wake release mutex is not poisoned")
            .recv_timeout(testing_aids::TEST_TIMEOUT)
            .expect("the test releases the blocked join wake");
    }
}

fn capture() -> (DecodedSnapshot, RuntimeSourceSnapshot) {
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).expect("the Seismograph snapshot encodes");
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).expect("the Seismograph snapshot decodes");
    let runtime_source = decoded
        .sources
        .iter()
        .find(|entry| entry.id == source::ID)
        .expect("the runtime source is registered");
    let runtime_snapshot = seismograph_runtime::snapshot::decode(&runtime_source.data).expect("the runtime source payload decodes");
    (decoded, runtime_snapshot)
}

fn runtime_ids() -> HashSet<RuntimeId> {
    let snapshot = seismograph::snapshot(SnapshotOptions::default()).expect("the Seismograph snapshot encodes");
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).expect("the Seismograph snapshot decodes");
    let Some(runtime_source) = decoded.sources.iter().find(|entry| entry.id == source::ID) else {
        return HashSet::new();
    };
    seismograph_runtime::snapshot::decode(&runtime_source.data)
        .expect("the runtime source payload decodes")
        .runtimes
        .into_iter()
        .map(|runtime| runtime.id)
        .collect()
}

fn new_runtime<'a>(snapshot: &'a RuntimeSourceSnapshot, previous: &HashSet<RuntimeId>) -> &'a RuntimeSnapshot {
    snapshot
        .runtimes
        .iter()
        .find(|runtime| runtime.name == "arty" && !previous.contains(&runtime.id))
        .expect("the scenario registers one new Arty runtime")
}

struct PendingDropPanic {
    started: Option<mpsc::Sender<()>>,
}

struct BorrowedSizedFuture<'a, const N: usize> {
    data: &'a [u8; N],
    padding: [u8; N],
}

impl<const N: usize> Future for BorrowedSizedFuture<'_, N> {
    type Output = usize;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(self.data.len() + self.padding.len())
    }
}

impl Future for PendingDropPanic {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if let Some(started) = self.started.take() {
            started.send(()).expect("the test retains the start receiver");
        }
        Poll::Pending
    }
}

impl Drop for PendingDropPanic {
    #[expect(clippy::panic, reason = "this scenario verifies destructor-panic classification")]
    fn drop(&mut self) {
        std::panic::panic_any("Seismograph cancellation destructor panic");
    }
}

#[expect(clippy::panic, reason = "the blocked join must be ready before its wake callback returns")]
fn exercise_join_notification_ordering() {
    let previous = runtime_ids();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the join-ordering runtime requires one available worker");
    let mut task = pin!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 42u32 }));
    let (notified, wake_started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        notified,
        release: Mutex::new(released),
    }));
    assert!(task.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    wake_started
        .recv_timeout(testing_aids::TEST_TIMEOUT)
        .expect("task completion wakes the blocked join");

    let Poll::Ready(Ok(value)) = task.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("the task result is ready before its wake callback returns");
    };
    assert_eq!(value, 42);
    let (_, snapshot) = capture();
    let runtime_snapshot = new_runtime(&snapshot, &previous);
    assert_eq!(
        (runtime_snapshot.counters.live_tasks, runtime_snapshot.counters.completed_tasks,),
        (0, 1)
    );

    release.send(()).expect("the blocked wake retains its release receiver");
    runtime.stop().expect("the join-ordering runtime stops cleanly");
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

fn exercise_unpolled_cancellation_runtime() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the unpolled-cancellation runtime requires one available worker");
    let Unaware(child) = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move {
            let child = cx.scheduler().spawn(async |_| std::future::pending::<()>().await);
            RuntimeOperations::from(&cx).request_stop();
            Unaware(child)
        })
        .join()
        .expect("the parent returns the unpolled child handle");
    runtime.stop().expect("the unpolled-cancellation runtime stops cleanly");
    assert!(child.join().expect_err("shutdown cancels the unpolled child").is_shutdown());
}

fn exercise_destructor_panic_runtime() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the destructor-panic runtime requires one available worker");
    let (started, ready) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started), |_, Unaware(started)| PendingDropPanic { started: Some(started) });
    ready
        .recv_timeout(testing_aids::TEST_TIMEOUT)
        .expect("the destructor-panic future reaches its pending poll");
    runtime.stop().expect("the destructor-panic runtime stops cleanly");
    assert!(
        task.join()
            .expect_err("the destructor panic occurs during shutdown cancellation")
            .is_shutdown()
    );
}

fn exercise_block_on_metadata_runtime() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the block-on metadata runtime requires one available worker");
    let small = [0u8; 1];
    assert_eq!(
        runtime
            .scheduler()
            .block_on(|_| BorrowedSizedFuture {
                data: &small,
                padding: [0; 1],
            })
            .expect("the small block_on future completes"),
        2
    );
    let large = [0u8; 257];
    assert_eq!(
        runtime
            .scheduler()
            .block_on(|_| BorrowedSizedFuture {
                data: &large,
                padding: [0; 257],
            })
            .expect("the large block_on future completes"),
        514
    );
    runtime.stop().expect("the block-on metadata runtime stops cleanly");
}

fn assert_default_recording_contract() {
    seismograph::recorder(Configuration::default());
    let previous = runtime_ids();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the default-recording runtime requires one available worker");
    runtime
        .scheduler()
        .spawn_anywhere((), |_, ()| async {})
        .join()
        .expect("the default-recording task completes");
    runtime.stop().expect("the default-recording runtime stops cleanly");

    let (decoded, snapshot) = capture();
    let runtime_snapshot = new_runtime(&snapshot, &previous);
    assert_eq!(
        (
            runtime_snapshot.counters.spawned_tasks,
            runtime_snapshot.counters.live_tasks,
            runtime_snapshot.counters.completed_tasks,
        ),
        (1, 0, 1)
    );
    assert!(
        !decoded
            .events
            .events
            .iter()
            .any(|event| { event.runtime().is_some_and(|payload| payload.runtime_id == runtime_snapshot.id) })
    );
}

fn assert_primary_events(decoded: &DecodedSnapshot, primary: &RuntimeSnapshot) {
    let events: Vec<_> = decoded
        .events
        .events
        .iter()
        .filter_map(|event| {
            let payload = event.runtime()?;
            (payload.runtime_id == primary.id).then_some((event.kind, payload.subject_id, payload.worker_id))
        })
        .collect();
    let count = |kind| events.iter().filter(|(event_kind, _, _)| *event_kind == kind).count();
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
    let spawned: Vec<_> = events
        .iter()
        .filter_map(|(kind, task, _)| (*kind == EventKind::TaskSpawned).then_some(*task))
        .collect();
    assert_eq!(spawned.len(), 5);
    let expected_terminals = [
        EventKind::TaskCompleted,
        EventKind::TaskCompleted,
        EventKind::TaskCompleted,
        EventKind::TaskPanicked,
        EventKind::TaskCanceled,
    ];
    for (task, expected_terminal) in spawned.into_iter().zip(expected_terminals) {
        let task_kinds: Vec<_> = events
            .iter()
            .filter_map(|(kind, subject, _)| (*subject == task).then_some(*kind))
            .collect();
        assert!(task_kinds.contains(&EventKind::TaskEnqueued));
        assert!(task_kinds.contains(&EventKind::TaskMaterialized));
        assert_eq!(
            task_kinds.iter().filter(|kind| **kind == EventKind::TaskPollStarted).count(),
            task_kinds.iter().filter(|kind| **kind == EventKind::TaskPollFinished).count()
        );
        let terminals: Vec<_> = task_kinds
            .into_iter()
            .filter(|kind| matches!(kind, EventKind::TaskCompleted | EventKind::TaskPanicked | EventKind::TaskCanceled))
            .collect();
        assert_eq!(terminals, [expected_terminal]);
    }
    assert!(
        events
            .iter()
            .filter(|(kind, _, _)| matches!(
                kind,
                EventKind::TaskMaterialized
                    | EventKind::TaskPollStarted
                    | EventKind::TaskPollFinished
                    | EventKind::TaskCompleted
                    | EventKind::TaskPanicked
                    | EventKind::TaskCanceled
            ))
            .all(|(_, _, worker)| *worker == Some(primary.workers[0].id))
    );
}

fn assert_unpolled_cancellation(decoded: &DecodedSnapshot, runtime: &RuntimeSnapshot) {
    assert_eq!(
        (
            runtime.counters.spawned_tasks,
            runtime.counters.completed_tasks,
            runtime.counters.canceled_tasks,
            runtime.counters.poll_count,
        ),
        (2, 1, 1, 1)
    );
    let task_events: Vec<_> = decoded
        .events
        .events
        .iter()
        .filter_map(|event| {
            let payload = event.runtime()?;
            (payload.runtime_id == runtime.id).then_some((event.kind, payload.subject_id))
        })
        .collect();
    let child = task_events
        .iter()
        .find_map(|(kind, task)| (*kind == EventKind::TaskCanceled).then_some(*task))
        .expect("the child task was canceled");
    let child_kinds: Vec<_> = task_events
        .iter()
        .filter_map(|(kind, task)| (*task == child).then_some(*kind))
        .collect();
    assert_eq!(child_kinds.len(), 5);
    for expected in [
        EventKind::TaskSpawned,
        EventKind::TaskEnqueued,
        EventKind::TaskReady,
        EventKind::TaskMaterialized,
        EventKind::TaskCanceled,
    ] {
        assert!(child_kinds.contains(&expected));
    }
    assert!(!child_kinds.contains(&EventKind::TaskPollStarted));
    assert!(!child_kinds.contains(&EventKind::TaskPollFinished));
}

fn assert_block_on_metadata(decoded: &DecodedSnapshot, runtime: &RuntimeSnapshot) {
    let spawned: Vec<_> = decoded
        .events
        .events
        .iter()
        .filter_map(|event| {
            let payload = event.runtime()?;
            (payload.runtime_id == runtime.id && event.kind == EventKind::TaskSpawned).then_some((payload.value_0, payload.value_1))
        })
        .collect();
    assert_eq!(spawned.len(), 2);
    assert_ne!(spawned[0].0, spawned[1].0);
    assert!(spawned[0].1 > 0);
    assert!(spawned[1].1 > spawned[0].1);
}

#[test]
fn public_spawn_paths_report_runtime_worker_task_and_poll_lifecycle() {
    let _reset = RecorderReset;
    seismograph::recorder(Configuration {
        runtime_tasks: RecordingPolicy::all(false),
        ..Configuration::default()
    });
    exercise_join_notification_ordering();
    let retained_scheduler = exercise_primary_runtime();
    exercise_stopped_runtime();
    let owner_drop_scheduler = exercise_owner_drop_runtime();
    exercise_unpolled_cancellation_runtime();
    exercise_destructor_panic_runtime();
    exercise_block_on_metadata_runtime();
    let (decoded, runtime_snapshot) = capture();
    let arty_runtimes: Vec<_> = runtime_snapshot.runtimes.iter().filter(|entry| entry.name == "arty").collect();

    let unique_ids: HashSet<_> = arty_runtimes.iter().map(|runtime| runtime.id).collect();
    assert_eq!(unique_ids.len(), arty_runtimes.len());
    assert_eq!(
        (
            arty_runtimes.iter().filter(|entry| entry.state == RuntimeState::Stopped).count(),
            arty_runtimes.iter().filter(|entry| entry.state == RuntimeState::Stopping).count(),
        ),
        (arty_runtimes.len() - 1, 1)
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
    let unpolled = arty_runtimes
        .iter()
        .find(|entry| {
            entry.state == RuntimeState::Stopped
                && entry.counters.spawned_tasks == 2
                && entry.counters.completed_tasks == 1
                && entry.counters.canceled_tasks == 1
        })
        .expect("the unpolled-cancellation runtime is retained");
    assert_unpolled_cancellation(&decoded, unpolled);
    let destructor_panic = arty_runtimes
        .iter()
        .find(|entry| entry.counters.spawned_tasks == 1 && entry.counters.panicked_tasks == 1)
        .expect("the destructor-panic runtime is retained");
    assert_eq!(
        (
            destructor_panic.counters.live_tasks,
            destructor_panic.counters.panicked_tasks,
            destructor_panic.counters.canceled_tasks,
        ),
        (0, 1, 0)
    );
    let block_on_metadata = arty_runtimes
        .iter()
        .find(|entry| entry.state == RuntimeState::Stopped && entry.counters.spawned_tasks == 2 && entry.counters.completed_tasks == 2)
        .expect("the block-on metadata runtime is retained");
    assert_block_on_metadata(&decoded, block_on_metadata);
    drop(retained_scheduler);
    drop(owner_drop_scheduler);
    assert_default_recording_contract();
}
