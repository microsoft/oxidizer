// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(feature = "rt")]

//! Public runtime and task lifecycle contracts exposed through Seismograph snapshots.

testing_aids::init_tracing!();

mod support;

use std::collections::HashSet;
use std::pin::{Pin, pin};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::task::{Context, Poll, Wake, Waker};
use std::time::Instant;

use arty::runtime::{Runtime, RuntimeId as ArtyRuntimeId, RuntimeOperations, WorkersPolicy};
use arty::task::Scheduler;
use events_once::Event;
use many_cpus::SystemHardware;
use seismograph::recorder::event::EventKind;
use seismograph::recorder::runtime::RuntimeId as SeismographRuntimeId;
use seismograph::recorder::{Configuration, EventBufferCapacity, RecordingPolicy};
use seismograph::snapshot::{DecodedSnapshot, SnapshotOptions};
use seismograph_runtime::snapshot::{Runtime as RuntimeSnapshot, RuntimeState, Snapshot as RuntimeSourceSnapshot, WorkerState, source};
use support::JoinHandleExt as _;
use thread_aware::Unaware;

struct RecorderReset;

// The scenario asserts runtime lifecycle events, not recorder ring capacity.
// This retains its complete per-worker event set without making Miri interpret
// the default 65,536-slot buffer for every short-lived worker.
const EVENT_CAPACITY: usize = 256;

impl Drop for RecorderReset {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

fn wait_for_notification(notification: &(Mutex<bool>, Condvar)) {
    let (started, ready) = notification;
    let mut started = started.lock().expect("the wake notification mutex is not poisoned");
    while !*started {
        started = ready.wait(started).expect("the wake notification mutex is not poisoned");
    }
}

struct BlockingWake {
    notified: Arc<(Mutex<bool>, Condvar)>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Wake for BlockingWake {
    fn wake(self: Arc<Self>) {
        let (started, ready) = self.notified.as_ref();
        *started.lock().expect("the wake notification mutex is not poisoned") = true;
        ready.notify_one();
        self.release
            .lock()
            .expect("the wake release mutex is not poisoned")
            .recv()
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

fn runtime_ids() -> HashSet<SeismographRuntimeId> {
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

fn new_runtime<'a>(snapshot: &'a RuntimeSourceSnapshot, previous: &HashSet<SeismographRuntimeId>) -> &'a RuntimeSnapshot {
    snapshot
        .runtimes
        .iter()
        .find(|runtime| runtime.name == "arty" && !previous.contains(&runtime.id))
        .expect("the scenario registers one new Arty runtime")
}

fn runtime_by_id(snapshot: &RuntimeSourceSnapshot, id: ArtyRuntimeId) -> &RuntimeSnapshot {
    snapshot
        .runtimes
        .iter()
        .find(|runtime| runtime.id.get() == id.get())
        .expect("the public Arty runtime identity matches its Seismograph snapshot")
}

struct PendingDropPanic {
    started: Option<mpsc::Sender<()>>,
}

struct QueuedDropPanic;

impl Drop for QueuedDropPanic {
    #[expect(clippy::panic, reason = "this scenario verifies queued factory destructor-panic classification")]
    fn drop(&mut self) {
        std::panic::panic_any("Seismograph queued factory destructor panic");
    }
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
    let (release_task, task_gate) = Event::boxed();
    let mut task = pin!(
        runtime
            .scheduler()
            .spawn_anywhere(Unaware(task_gate), |_, Unaware(task_gate)| async move {
                task_gate.await.expect("the test releases task completion");
                42u32
            })
    );
    let wake_started = Arc::new((Mutex::new(false), Condvar::new()));
    let (release, released) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        notified: Arc::clone(&wake_started),
        release: Mutex::new(released),
    }));
    assert!(task.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    release_task.send(());
    wait_for_notification(&wake_started);

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

#[expect(clippy::panic, reason = "the canceled join must be ready before its wake callback returns")]
fn exercise_cancellation_notification_ordering() {
    let previous = runtime_ids();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the cancellation-ordering runtime requires one available worker");
    let mut task = pin!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| async { std::future::pending::<()>().await })
    );
    let wake_started = Arc::new((Mutex::new(false), Condvar::new()));
    let (release, released) = mpsc::channel();
    let waker = Waker::from(Arc::new(BlockingWake {
        notified: Arc::clone(&wake_started),
        release: Mutex::new(released),
    }));
    assert!(task.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    RuntimeOperations::from(&runtime).request_stop();
    wait_for_notification(&wake_started);

    let Poll::Ready(Err(error)) = task.as_mut().poll(&mut Context::from_waker(Waker::noop())) else {
        panic!("the canceled join is ready before its wake callback returns");
    };
    assert!(error.is_shutdown());
    let (_, snapshot) = capture();
    let runtime_snapshot = new_runtime(&snapshot, &previous);
    assert_eq!(
        (runtime_snapshot.counters.live_tasks, runtime_snapshot.counters.canceled_tasks,),
        (0, 1)
    );

    release.send(()).expect("the blocked wake retains its release receiver");
    runtime.stop().expect("the cancellation-ordering runtime stops cleanly");
}

#[expect(clippy::panic, reason = "this scenario verifies Seismograph panic classification")]
fn exercise_primary_runtime() -> Scheduler {
    let previous = runtime_ids();
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
            let direct = cx.scheduler().spawn(async |_| {
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
            });
            let anywhere = cx.scheduler().spawn_anywhere((), |()| async {});
            direct.await.expect("the direct worker-local child completes");
            anywhere.await.expect("the runtime-placed child completes");
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
    assert!(
        runtime
            .scheduler()
            .spawn_anywhere((), |_, ()| -> std::future::Ready<()> {
                std::panic::panic_any("seismograph factory panic contract")
            })
            .join()
            .expect_err("the intentional factory panic reaches the join handle")
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
    let (decoded, snapshot) = capture();
    let primary = new_runtime(&snapshot, &previous);
    assert_eq!(
        (
            primary.counters.live_tasks,
            primary.counters.completed_tasks,
            primary.counters.panicked_tasks,
            primary.counters.canceled_tasks,
        ),
        (0, 4, 2, 1)
    );
    assert!(primary.counters.poll_count >= 7);
    let nested: Vec<_> = decoded
        .events
        .events
        .iter()
        .filter_map(|event| {
            let payload = event.runtime()?;
            (event.kind == EventKind::TaskSpawned && payload.runtime_id == primary.id && payload.related_id != 0).then_some(payload)
        })
        .collect();
    assert_eq!(nested.len(), 2, "both task-originated spawn paths retain the active parent");
    for nested in nested {
        assert!(
            decoded.events.events.iter().any(|event| {
                event.runtime().is_some_and(|payload| {
                    event.kind == EventKind::TaskSpawned
                        && payload.runtime_id == primary.id
                        && payload.subject_id == nested.related_id
                        && payload.related_id == 0
                })
            }),
            "the nested task's parent identity belongs to the same runtime"
        );
    }
    assert_primary_events(&decoded, primary);
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
    let previous = runtime_ids();
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
    let deadline = Instant::now() + testing_aids::TEST_TIMEOUT;
    loop {
        let (decoded, snapshot) = capture();
        let runtime = new_runtime(&snapshot, &previous);
        if runtime.state == RuntimeState::Stopped
            && decoded.events.events.iter().any(|event| {
                event.kind == EventKind::RuntimeStopped && event.runtime().is_some_and(|payload| payload.runtime_id == runtime.id)
            })
        {
            break;
        }
        assert!(Instant::now() < deadline, "owner-drop shutdown must report stopped");
        std::thread::yield_now();
    }
    owner_drop_scheduler
}

fn exercise_unpolled_cancellation_runtime() {
    let previous = runtime_ids();
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
    let (decoded, snapshot) = capture();
    assert_unpolled_cancellation(&decoded, new_runtime(&snapshot, &previous));
}

fn exercise_destructor_panic_runtime() {
    let previous = runtime_ids();
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
    let (_, snapshot) = capture();
    let runtime = new_runtime(&snapshot, &previous);
    assert_eq!(
        (
            runtime.counters.live_tasks,
            runtime.counters.panicked_tasks,
            runtime.counters.canceled_tasks,
        ),
        (0, 1, 0)
    );
}

fn exercise_queued_factory_destructor_panic_runtime() {
    let previous = runtime_ids();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the queued-factory runtime requires one available worker");
    let (started, worker_blocked) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let blocker = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, released)), |_, Unaware((started, released))| async move {
            started.send(()).expect("the test retains the blocker start receiver");
            released
                .recv_timeout(testing_aids::TEST_TIMEOUT)
                .expect("the test releases the blocked worker");
        });
    worker_blocked
        .recv_timeout(testing_aids::TEST_TIMEOUT)
        .expect("the first task blocks its worker");
    let queued = runtime
        .scheduler()
        .spawn_anywhere(Unaware(QueuedDropPanic), |_, Unaware(capture)| async move { drop(capture) });
    RuntimeOperations::from(&runtime).request_stop();
    release.send(()).expect("the blocked worker retains its release receiver");
    blocker.join().expect("the blocker completes after release");
    runtime.stop().expect("the queued-factory runtime stops cleanly");
    assert!(queued.join().expect_err("shutdown discards the queued factory").is_shutdown());
    let (_, snapshot) = capture();
    let runtime = new_runtime(&snapshot, &previous);
    assert_eq!(
        (
            runtime.counters.spawned_tasks,
            runtime.counters.completed_tasks,
            runtime.counters.canceled_tasks,
            runtime.counters.panicked_tasks,
        ),
        (2, 1, 0, 1)
    );
}

fn exercise_block_on_metadata_runtime() {
    let previous = runtime_ids();
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
    let (decoded, snapshot) = capture();
    assert_block_on_metadata(&decoded, new_runtime(&snapshot, &previous));
}

fn exercise_multiworker_placement_runtime() {
    if SystemHardware::current().processors().len() < 2 {
        return;
    }
    let previous = runtime_ids();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .build()
        .expect("two available processors are required for placement attribution");
    let scheduler = runtime
        .scheduler()
        .block_on(async |cx| cx.scheduler().clone())
        .expect("the runtime returns a worker-local scheduler");
    for task in scheduler.spawn_everywhere((), |()| async {}) {
        task.join().expect("each worker-local task completes");
    }
    runtime.stop().expect("the multi-worker runtime stops cleanly");

    let (decoded, snapshot) = capture();
    let runtime = new_runtime(&snapshot, &previous);
    let workers = runtime.workers.iter().map(|worker| worker.id).collect::<HashSet<_>>();
    assert_eq!(workers.len(), 2);
    for kind in [EventKind::TaskMaterialized, EventKind::TaskCompleted] {
        let attributed = decoded
            .events
            .events
            .iter()
            .filter(|event| event.kind == kind)
            .filter_map(|event| {
                let payload = event.runtime()?;
                (payload.runtime_id == runtime.id).then_some(payload.worker_id)
            })
            .flatten()
            .collect::<HashSet<_>>();
        assert_eq!(attributed, workers);
    }
}

fn exercise_runtime_identity_and_idle_worker() {
    let first = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the first identity runtime requires one available worker");
    let second = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .build()
        .expect("the second identity runtime requires one available worker");
    assert_ne!(first.id(), second.id());
    assert_eq!(first.id().to_string(), first.id().get().to_string());

    for id in [first.id(), second.id()] {
        let deadline = Instant::now() + testing_aids::TEST_TIMEOUT;
        loop {
            let (_, snapshot) = capture();
            if runtime_by_id(&snapshot, id).workers[0].state == WorkerState::Parked {
                break;
            }
            assert!(Instant::now() < deadline, "idle Arty workers report the parked state");
            std::thread::yield_now();
        }
    }

    let first_id = first.id();
    let second_id = second.id();
    first.stop().expect("the first identity runtime stops cleanly");
    second.stop().expect("the second identity runtime stops cleanly");
    let (decoded, snapshot) = capture();
    for id in [first_id, second_id] {
        assert_eq!(runtime_by_id(&snapshot, id).state, RuntimeState::Stopped);
        let events: Vec<_> = decoded
            .events
            .events
            .iter()
            .filter_map(|event| {
                let payload = event.runtime()?;
                (payload.runtime_id.get() == id.get()).then_some(event.kind)
            })
            .collect();
        assert!(events.contains(&EventKind::WorkerParked));
        assert!(events.contains(&EventKind::WorkerUnparked));
    }
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
    assert_eq!(count(EventKind::RuntimeStopped), 1, "primary events: {events:?}");
    assert_eq!(count(EventKind::WorkerStarted), 1);
    assert_eq!(count(EventKind::WorkerStopped), 1);
    assert_eq!(count(EventKind::TaskEnqueued), 7);
    assert_eq!(count(EventKind::TaskMaterialized), 7);
    assert_eq!(count(EventKind::TaskCompleted), 4);
    assert_eq!(count(EventKind::TaskPanicked), 2);
    assert_eq!(count(EventKind::TaskCanceled), 1);
    assert_eq!(count(EventKind::TaskPollStarted), count(EventKind::TaskPollFinished));
    let spawned: Vec<_> = events
        .iter()
        .filter_map(|(kind, task, _)| (*kind == EventKind::TaskSpawned).then_some(*task))
        .collect();
    assert_eq!(spawned.len(), 7);
    let expected_terminals = [
        EventKind::TaskCompleted,
        EventKind::TaskCompleted,
        EventKind::TaskCompleted,
        EventKind::TaskCompleted,
        EventKind::TaskPanicked,
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
        event_capacity_per_thread: EventBufferCapacity::new(EVENT_CAPACITY).expect("the test event capacity is a supported power of two"),
        ..Configuration::default()
    });
    exercise_join_notification_ordering();
    exercise_cancellation_notification_ordering();
    let retained_scheduler = exercise_primary_runtime();
    exercise_stopped_runtime();
    let owner_drop_scheduler = exercise_owner_drop_runtime();
    exercise_unpolled_cancellation_runtime();
    exercise_destructor_panic_runtime();
    exercise_queued_factory_destructor_panic_runtime();
    exercise_block_on_metadata_runtime();
    exercise_multiworker_placement_runtime();
    exercise_runtime_identity_and_idle_worker();
    let (_, runtime_snapshot) = capture();
    let arty_runtimes: Vec<_> = runtime_snapshot.runtimes.iter().filter(|entry| entry.name == "arty").collect();

    let unique_ids: HashSet<_> = arty_runtimes.iter().map(|runtime| runtime.id).collect();
    assert_eq!(unique_ids.len(), arty_runtimes.len());
    assert!(arty_runtimes.iter().all(|entry| entry.state == RuntimeState::Stopped));
    assert!(arty_runtimes.iter().all(|entry| {
        !entry.workers.is_empty()
            && entry
                .workers
                .iter()
                .all(|worker| worker.thread_id.is_some() && worker.processor_index.is_some())
    }));

    drop(retained_scheduler);
    drop(owner_drop_scheduler);
    assert_default_recording_contract();
}
