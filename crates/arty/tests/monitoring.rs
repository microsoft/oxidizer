// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public monitoring integration contracts for the Arty runtime.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

use std::future::pending;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard, mpsc};
use std::task::{Context, Poll};

use arty::runtime::Runtime;
use arty::runtime::hook::TaskPanic;
use arty::runtime::policy::{BlockingPoolPolicy, MonitorPolicy, WorkersPolicy};
use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, Policy};
use seismograph::snapshot::{DecodedSnapshot, SnapshotOptions};
use seismograph_runtime::snapshot::{Runtime as RuntimeSnapshot, TaskActivityState, TaskCollection, TaskMetricsScope};
use testing_aids::{TEST_TIMEOUT, is_mutation_testing};
use thread_aware::Unaware;

static RECORDING: Mutex<()> = Mutex::new(());

struct RecorderGuard {
    _lock: MutexGuard<'static, ()>,
}

impl RecorderGuard {
    fn new(record_tasks: bool) -> Self {
        let lock = RECORDING.lock().unwrap();
        seismograph::recorder(if record_tasks {
            Configuration {
                runtime_tasks: Policy::all(false),
                ..Configuration::default()
            }
        } else {
            Configuration::default()
        });
        Self { _lock: lock }
    }
}

impl Drop for RecorderGuard {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

fn runtime(name: &str, workers: usize) -> Runtime {
    arty::runtime::test_util::with_mock_monitor_io(|| {
        Runtime::builder()
            .name(name)
            .workers(WorkersPolicy::exactly(workers))
            .blocking_pool(BlockingPoolPolicy::shared().max(1))
            .monitor(MonitorPolicy::enabled())
            .on(TaskPanic::ignore())
            .build()
            .unwrap()
    })
}

fn process_snapshot() -> DecodedSnapshot {
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    seismograph::snapshot::decode(bytes.as_bytes()).unwrap()
}

fn runtime_snapshots(snapshot: &DecodedSnapshot) -> Vec<RuntimeSnapshot> {
    snapshot
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .map_or_else(Vec::new, |source| {
            seismograph_runtime::snapshot::decode(&source.data).unwrap().runtimes
        })
}

fn capture_runtime(name: &str) -> RuntimeSnapshot {
    let started = std::time::Instant::now();
    loop {
        if let Some(runtime) = runtime_snapshots(&process_snapshot())
            .into_iter()
            .find(|runtime| runtime.name == name)
            && runtime
                .workers
                .iter()
                .all(|worker| !matches!(worker.task_collection, TaskCollection::Busy | TaskCollection::TimedOut))
        {
            return runtime;
        }
        assert!(
            is_mutation_testing() || started.elapsed() < TEST_TIMEOUT,
            "runtime workers did not answer the monitoring capture"
        );
        std::thread::yield_now();
    }
}

fn receive<T>(receiver: &mpsc::Receiver<T>) -> T {
    if is_mutation_testing() {
        receiver.recv().unwrap()
    } else {
        receiver.recv_timeout(TEST_TIMEOUT).unwrap()
    }
}

struct PendingBody<const N: usize> {
    started: Option<mpsc::Sender<()>>,
    _payload: [u8; N],
}

impl<const N: usize> PendingBody<N> {
    fn new(started: mpsc::Sender<()>) -> Self {
        Self {
            started: Some(started),
            _payload: [0; N],
        }
    }
}

impl<const N: usize> Future for PendingBody<N> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if let Some(started) = self.started.take() {
            started.send(()).unwrap();
        }
        Poll::Pending
    }
}

#[test]
fn disabled_monitoring_registers_no_runtime_source() {
    let _recording = RecorderGuard::new(false);
    let name = "integration-disabled-monitoring";
    let runtime = Runtime::builder().name(name).monitor(MonitorPolicy::disabled()).build().unwrap();

    assert_eq!(runtime.scheduler().block_on(async |_| 42).unwrap(), 42);
    assert!(runtime_snapshots(&process_snapshot()).iter().all(|runtime| runtime.name != name));
    runtime.stop().unwrap();
}

#[test]
fn idle_monitoring_exposes_only_native_task_state() {
    let _recording = RecorderGuard::new(false);
    let name = "integration-idle-metadata";
    let runtime = runtime(name, 1);
    let (started, ready) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started), |_, Unaware(started)| PendingBody::<64>::new(started));
    receive(&ready);

    let first = capture_runtime(name);
    let second = capture_runtime(name);
    runtime.stop().unwrap();
    assert!(futures::executor::block_on(task).unwrap_err().is_shutdown());

    assert_eq!(first.tasks.len(), 1);
    let first_task = &first.tasks[0];
    let second_task = &second.tasks[0];
    assert_eq!(
        (
            first_task.parent,
            first_task.activity.unwrap().state,
            first_task.spawned_at,
            first_task.future_size_bytes,
            first_task.metrics_scope,
            first_task.last_worker_id.is_some(),
        ),
        (None, TaskActivityState::Waiting, None, None, TaskMetricsScope::Uncollected, true,)
    );
    assert_eq!(
        (second_task.id, second_task.type_descriptor),
        (first_task.id, first_task.type_descriptor)
    );
}

#[test]
fn remote_spawn_uses_origin_metadata_and_destination_execution() {
    let _recording = RecorderGuard::new(true);
    let name = "integration-remote-origin";
    let runtime = runtime(name, 2);
    let (started, ready) = mpsc::channel();
    let (spawned, children) = mpsc::channel();
    let parent = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, spawned)), |cx, Unaware((started, spawned))| async move {
            let destination = cx.control().workers().find(|worker| worker != cx.thread()).unwrap();
            let at = cx.scheduler().spawn_at(&destination, Unaware(started.clone()), |Unaware(started)| {
                PendingBody::<64>::new(started)
            });
            let anywhere = cx
                .scheduler()
                .spawn_anywhere(Unaware(started), |Unaware(started)| PendingBody::<128>::new(started));
            spawned.send((at, anywhere)).unwrap();
            pending::<()>().await;
        });
    let (at, anywhere) = receive(&children);
    receive(&ready);
    receive(&ready);

    let native = capture_runtime(name);
    runtime.stop().unwrap();
    assert!(futures::executor::block_on(parent).unwrap_err().is_shutdown());
    assert!(futures::executor::block_on(at).unwrap_err().is_shutdown());
    assert!(futures::executor::block_on(anywhere).unwrap_err().is_shutdown());
    let root = native.tasks.iter().find(|task| task.parent.is_none()).unwrap();
    let children = native.tasks.iter().filter(|task| task.parent == Some(root.id)).collect::<Vec<_>>();
    assert_eq!(children.len(), 2);
    assert_eq!(
        children
            .iter()
            .map(|task| task.future_size_bytes.unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        [
            u64::try_from(size_of::<PendingBody<64>>()).unwrap(),
            u64::try_from(size_of::<PendingBody<128>>()).unwrap(),
        ]
        .into_iter()
        .collect()
    );

    let snapshot = process_snapshot();
    for child in children {
        let execution_worker = child.last_worker_id.unwrap();
        let spawn = snapshot
            .events
            .events
            .iter()
            .find(|event| {
                event.kind == EventKind::TaskSpawned
                    && event
                        .runtime()
                        .is_some_and(|payload| payload.runtime_id == native.id && payload.subject_id == child.id.get())
            })
            .unwrap();
        let payload = spawn.runtime().unwrap();
        assert_eq!(
            (payload.worker_id, payload.related_id, child.spawned_at),
            (None, root.id.get(), Some(spawn.timestamp))
        );
        assert!(snapshot.events.events.iter().any(|event| {
            event.kind == EventKind::TaskPollStarted
                && event.runtime().is_some_and(|payload| {
                    payload.runtime_id == native.id && payload.subject_id == child.id.get() && payload.worker_id == Some(execution_worker)
                })
        }));
    }
}

#[test]
fn cross_runtime_spawn_records_only_in_the_destination_runtime() {
    let _recording = RecorderGuard::new(true);
    let first_name = "integration-cross-runtime-origin";
    let second_name = "integration-cross-runtime-destination";
    let first = runtime(first_name, 1);
    let second = runtime(second_name, 1);
    let second_scheduler = second.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (started, ready) = mpsc::channel();
    let (spawned, child_handle) = mpsc::channel();
    let parent = first.scheduler().spawn_anywhere(
        Unaware((second_scheduler, started, spawned)),
        |_, Unaware((second_scheduler, started, spawned))| async move {
            let child = second_scheduler.spawn_anywhere(Unaware(started), |Unaware(started)| PendingBody::<32>::new(started));
            spawned.send(child).unwrap();
            pending::<()>().await;
        },
    );
    let child = receive(&child_handle);
    receive(&ready);

    let first_native = capture_runtime(first_name);
    let second_native = capture_runtime(second_name);
    let child_task = &second_native.tasks[0];
    let parent_task = &first_native.tasks[0];
    assert_eq!(child_task.parent, Some(parent_task.id));

    first.stop().unwrap();
    second.stop().unwrap();
    assert!(futures::executor::block_on(parent).unwrap_err().is_shutdown());
    assert!(futures::executor::block_on(child).unwrap_err().is_shutdown());

    let snapshot = process_snapshot();
    let spawns = snapshot
        .events
        .events
        .iter()
        .filter(|event| {
            event.kind == EventKind::TaskSpawned && event.runtime().is_some_and(|payload| payload.subject_id == child_task.id.get())
        })
        .collect::<Vec<_>>();
    assert_eq!(spawns.len(), 1);
    assert_eq!(
        (spawns[0].runtime().unwrap().runtime_id, spawns[0].runtime().unwrap().worker_id),
        (second_native.id, None)
    );
}

#[test]
fn blocking_monitoring_distinguishes_submission_execution_and_queue_state() {
    let _recording = RecorderGuard::new(true);
    let name = "integration-blocking-state";
    let runtime = runtime(name, 1);
    let (started, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let running = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        receive(&released);
        1
    });
    receive(&ready);
    let queued_first = runtime.scheduler().spawn_blocking(|| 2);
    let queued_second = runtime.scheduler().spawn_blocking(|| 3);

    let native = capture_runtime(name);
    release.send(()).unwrap();
    assert_eq!(
        (
            futures::executor::block_on(running).unwrap(),
            futures::executor::block_on(queued_first).unwrap(),
            futures::executor::block_on(queued_second).unwrap(),
        ),
        (1, 2, 3)
    );
    runtime.stop().unwrap();

    let running = native
        .tasks
        .iter()
        .find(|task| task.activity.unwrap().state == TaskActivityState::Running)
        .unwrap();
    let queued = native
        .tasks
        .iter()
        .filter(|task| task.activity.unwrap().state == TaskActivityState::Ready)
        .collect::<Vec<_>>();
    assert_eq!(queued.len(), 2, "the native Ready rows are the blocking queue depth");
    let activity = running.activity.unwrap();
    assert_eq!(activity.poll_worker_id, running.last_worker_id);
    assert!(running.last_worker_id.is_some());
    let poll_started_at = activity.poll_started_at.unwrap();
    assert!(activity.observed_at >= poll_started_at);
    for task in &queued {
        let activity = task.activity.unwrap();
        assert_eq!((task.last_worker_id, activity.poll_worker_id), (None, None));
        assert_eq!((activity.ready_since, activity.queued_since), (task.spawned_at, task.spawned_at));
        assert!(activity.observed_at >= activity.queued_since.unwrap());
    }

    let snapshot = process_snapshot();
    for task in std::iter::once(running).chain(queued.iter().copied()) {
        let spawn = snapshot
            .events
            .events
            .iter()
            .find(|event| {
                event.kind == EventKind::TaskSpawned
                    && event
                        .runtime()
                        .is_some_and(|payload| payload.runtime_id == native.id && payload.subject_id == task.id.get())
            })
            .unwrap();
        assert_eq!((spawn.runtime().unwrap().worker_id, task.spawned_at), (None, Some(spawn.timestamp)));
    }
    let events = snapshot
        .events
        .events
        .iter()
        .filter(|event| {
            event
                .runtime()
                .is_some_and(|payload| payload.runtime_id == native.id && payload.subject_id == running.id.get())
        })
        .collect::<Vec<_>>();
    let started = events.iter().find(|event| event.kind == EventKind::TaskPollStarted).unwrap();
    let finished_index = events.iter().position(|event| event.kind == EventKind::TaskPollFinished).unwrap();
    let finished = events[finished_index];
    let completed_index = events.iter().position(|event| event.kind == EventKind::TaskCompleted).unwrap();
    assert_eq!(
        finished.timestamp.ticks() - started.timestamp.ticks(),
        finished.runtime().unwrap().value_0
    );
    assert!(finished_index < completed_index);
}

#[test]
fn recorded_polls_precede_terminal_outcomes_and_report_duration() {
    let _recording = RecorderGuard::new(true);
    let name = "integration-poll-lifecycle";
    let runtime = runtime(name, 1);
    let id = capture_runtime(name).id;
    assert_eq!(
        futures::executor::block_on(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 })).unwrap(),
        7
    );
    assert!(
        futures::executor::block_on(
            runtime
                .scheduler()
                .spawn_anywhere((), |_, ()| async { panic!("integration panic") }),
        )
        .unwrap_err()
        .is_panic()
    );
    let (started, ready) = mpsc::channel();
    let canceled = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started), |_, Unaware(started)| async move {
            started.send(()).unwrap();
            pending::<()>().await;
        });
    receive(&ready);
    runtime.stop().unwrap();
    assert!(futures::executor::block_on(canceled).unwrap_err().is_shutdown());

    let snapshot = process_snapshot();
    let events = snapshot
        .events
        .events
        .iter()
        .filter(|event| event.runtime().is_some_and(|payload| payload.runtime_id == id))
        .collect::<Vec<_>>();
    for (index, event) in events.iter().enumerate() {
        if event.kind == EventKind::TaskPollFinished {
            let payload = event.runtime().unwrap();
            let started = events[..index]
                .iter()
                .rev()
                .find(|candidate| {
                    candidate.kind == EventKind::TaskPollStarted
                        && candidate.runtime().is_some_and(|started| started.subject_id == payload.subject_id)
                })
                .unwrap();
            assert_eq!(event.timestamp.ticks() - started.timestamp.ticks(), payload.value_0);
        }
        if matches!(event.kind, EventKind::TaskCompleted | EventKind::TaskPanicked) {
            let payload = event.runtime().unwrap();
            assert!(
                events[..index].iter().any(|candidate| {
                    candidate.kind == EventKind::TaskPollFinished
                        && candidate
                            .runtime()
                            .is_some_and(|finished| finished.subject_id == payload.subject_id)
                }),
                "terminal outcomes must follow the final poll"
            );
        }
    }
    assert_eq!(
        (
            events.iter().filter(|event| event.kind == EventKind::TaskCompleted).count(),
            events.iter().filter(|event| event.kind == EventKind::TaskPanicked).count(),
            events.iter().filter(|event| event.kind == EventKind::TaskCanceled).count(),
        ),
        (1, 1, 1)
    );
}
