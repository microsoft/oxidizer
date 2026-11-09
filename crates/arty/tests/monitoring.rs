// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arty supplies native task state independently of event recording.

// Collection uses real worker-response deadlines. Native and careful tests cover
// this integration; the executor retains direct storage coverage under Miri.
#![cfg(all(feature = "rt", feature = "monitor", feature = "test-util", not(miri)))]
#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

mod support;

use std::future::{pending, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::task::{Context, Poll};
use std::time::Duration;

use arty::runtime::{BlockingPoolPolicy, MonitorPolicy, PanicPolicy, Runtime, WorkersPolicy};
use seismograph::recorder::event::{EventClass, EventKind};
use seismograph::recorder::{Configuration, Policy};
use seismograph::snapshot::SnapshotOptions;
use seismograph_protocol::discovery::Directory;
use seismograph_protocol::io::{IoOperation, MonitorIo};
use seismograph_protocol::message::{RecordingConfiguration, RecordingPolicy, Request, Response};
use seismograph_protocol::transport::Connection;
use seismograph_protocol::{read_response, write_request};
use seismograph_runtime::snapshot::{
    CaptureOptions, Runtime as RuntimeSnapshot, RuntimeState, TaskActivityState, TaskCollection, TaskMetricsScope,
};
use seismograph_runtime::worker::WorkerRole;
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

static RECORDING: Mutex<()> = Mutex::new(());

struct RecorderGuard {
    _lock: MutexGuard<'static, ()>,
}

impl RecorderGuard {
    fn new() -> Self {
        let lock = RECORDING.lock().unwrap();
        seismograph::recorder(Configuration::default());
        Self { _lock: lock }
    }
}

impl Drop for RecorderGuard {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

fn runtime(name: &str, io: &MonitorIo) -> Runtime {
    Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .monitor(MonitorPolicy::enabled().name(name).io(io.clone()))
        .panic(PanicPolicy::ignore())
        .build()
        .unwrap()
}

fn runtimes() -> Vec<RuntimeSnapshot> {
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    snapshot
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .map_or_else(Vec::new, |source| {
            seismograph_runtime::snapshot::decode(&source.data).unwrap().runtimes
        })
}

fn capture(runtime: &Runtime) -> RuntimeSnapshot {
    runtime
        .diagnostics()
        .unwrap()
        .capture(CaptureOptions {
            timeout: Duration::from_secs(5),
            ..CaptureOptions::default()
        })
        .unwrap()
}

fn names(io: &MonitorIo) -> Vec<String> {
    let directory = Directory::current(io).unwrap();
    let mut names = directory
        .descriptors()
        .unwrap()
        .map(|descriptor| descriptor.unwrap().name)
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn disabled_monitoring_registers_no_runtime_or_endpoint() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = Runtime::builder()
        .monitor(MonitorPolicy::disabled().name("disabled-monitor").io(io.clone()))
        .build()
        .unwrap();

    let value = runtime.scheduler().block_on(async |_| 42).unwrap();
    let registered = runtimes().iter().any(|runtime| runtime.name == "disabled-monitor");
    runtime.stop().unwrap();

    assert_eq!((value, registered, names(&io)), (42, false, Vec::new()));
}

#[test]
fn local_and_remote_tasks_are_collected_without_recording() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("native-tasks", &io);
    let (started, ready) = mpsc::channel();
    let parent = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started), |cx, Unaware(started)| async move {
            cx.scheduler()
                .spawn(move |_| {
                    let mut started = Some(started);
                    poll_fn(move |_| {
                        if let Some(started) = started.take() {
                            started.send(()).unwrap();
                        }
                        Poll::<()>::Pending
                    })
                })
                .await
                .unwrap();
        });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();

    let snapshot = capture(&runtime);
    runtime.stop().unwrap();
    assert!(parent.join().unwrap_err().is_shutdown());
    let observed = snapshot
        .tasks
        .iter()
        .map(|task| {
            (
                task.activity.unwrap().state,
                task.metrics_scope,
                task.spawned_at,
                task.last_worker_id.is_some(),
                task.future_size_bytes.is_some(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        observed,
        vec![(TaskActivityState::Waiting, TaskMetricsScope::Uncollected, None, true, true); 2]
    );
    assert!(
        snapshot
            .workers
            .iter()
            .all(|worker| worker.task_collection == TaskCollection::Complete)
    );
}

#[test]
fn task_identity_survives_repeated_collection_and_recording_changes() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("stable-identities", &io);
    let (started, ready) = mpsc::channel();
    let task = runtime.scheduler().spawn_anywhere(Unaware(started), |_, Unaware(started)| {
        let mut started = Some(started);
        poll_fn(move |_| {
            if let Some(started) = started.take() {
                started.send(()).unwrap();
            }
            Poll::<()>::Pending
        })
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let first = capture(&runtime);
    runtime.scheduler().block_on(async |_| ()).unwrap();

    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let second = capture(&runtime);
    runtime.scheduler().block_on(async |_| ()).unwrap();
    seismograph::recorder(Configuration::default());
    let third = capture(&runtime);
    runtime.stop().unwrap();
    assert!(task.join().unwrap_err().is_shutdown());

    for snapshot in [&first, &second, &third] {
        assert_eq!(snapshot.tasks.len(), 1);
        assert!(
            snapshot
                .workers
                .iter()
                .all(|worker| worker.task_collection == TaskCollection::Complete)
        );
    }
    assert_eq!(
        (second.tasks[0].id, third.tasks[0].id, second.tasks[0].spawned_at),
        (first.tasks[0].id, first.tasks[0].id, None)
    );
    assert_eq!(
        (second.tasks[0].type_descriptor, third.tasks[0].type_descriptor),
        (first.tasks[0].type_descriptor, first.tasks[0].type_descriptor)
    );
}

#[test]
fn queued_and_running_blocking_tasks_come_from_native_pool_storage() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("blocking-tasks", &io);
    let (started, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let first = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        released.recv_timeout(TEST_TIMEOUT).unwrap();
        1
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let second = runtime.scheduler().spawn_blocking(|| 2);

    let snapshot = capture(&runtime);
    release.send(()).unwrap();
    assert_eq!((first.join().unwrap(), second.join().unwrap()), (1, 2));
    runtime.stop().unwrap();
    let states = snapshot
        .tasks
        .iter()
        .map(|task| {
            let worker = snapshot
                .workers
                .iter()
                .find(|worker| Some(worker.id) == task.last_worker_id)
                .unwrap();
            (worker.role, task.activity.unwrap().state, task.metrics_scope)
        })
        .collect::<Vec<_>>();

    assert_eq!(
        states,
        [
            (WorkerRole::Blocking, TaskActivityState::Running, TaskMetricsScope::Uncollected),
            (WorkerRole::Blocking, TaskActivityState::Ready, TaskMetricsScope::Uncollected),
        ]
    );
}

#[test]
fn draining_queued_blocking_tasks_does_not_report_worker_resumes() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("blocking-queue-resumes", &io);
    let (started, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let first = runtime.scheduler().spawn_blocking(move || {
        started.send(seismograph::recorder::thread_id()).unwrap();
        released.recv_timeout(TEST_TIMEOUT).unwrap();
    });
    let thread_id = ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let second = runtime.scheduler().spawn_blocking(|| ());
    let third = runtime.scheduler().spawn_blocking(|| ());
    let (started, ready) = mpsc::channel();
    let (finish, finished) = mpsc::channel();
    let last = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        finished.recv_timeout(TEST_TIMEOUT).unwrap();
    });
    release.send(()).unwrap();
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let runtime_snapshot = capture(&runtime);
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    finish.send(()).unwrap();
    first.join().unwrap();
    second.join().unwrap();
    third.join().unwrap();
    last.join().unwrap();
    runtime.stop().unwrap();

    let worker = runtime_snapshot
        .workers
        .iter()
        .find(|worker| worker.role == WorkerRole::Blocking)
        .unwrap();
    let events = snapshot
        .events
        .events
        .iter()
        .filter(|event| {
            event
                .runtime()
                .is_some_and(|payload| payload.runtime_id == runtime_snapshot.id && payload.worker_id == Some(worker.id))
        })
        .skip_while(|event| event.kind != EventKind::TaskPollStarted)
        .collect::<Vec<_>>();
    let polls = events.iter().filter(|event| event.kind == EventKind::TaskPollStarted).count();
    let transitions = events
        .iter()
        .filter_map(|event| match event.kind {
            EventKind::WorkerParked | EventKind::WorkerUnparked => Some(event.kind),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!((worker.thread_id, polls, transitions), (Some(thread_id), 4, Vec::new()));
}

#[test]
fn stopped_runtime_does_not_record_new_blocking_tasks() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("stopped-blocking-submission", &io);
    let id = runtime.diagnostics().unwrap().id();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    runtime.stop().unwrap();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let outcome = scheduler.spawn_blocking(|| panic!("a stopped pool must not run a function")).join();
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let spawned = snapshot
        .events
        .events
        .iter()
        .filter(|event| event.kind == EventKind::TaskSpawned && event.runtime().is_some_and(|payload| payload.runtime_id == id))
        .count();

    assert_eq!((outcome.unwrap_err().is_shutdown(), spawned), (true, 0));
}

#[test]
fn recording_enabled_during_blocking_execution_reports_real_outcomes() {
    let _recording = RecorderGuard::new();
    for (panics, stops) in [(false, false), (true, false), (false, true)] {
        seismograph::recorder(Configuration::default());
        let (io, _) = MonitorIo::new_mocked();
        let runtime = runtime("late-blocking-recording", &io);
        let id = runtime.diagnostics().unwrap().id();
        let control = support::runtime_control(&runtime);
        let (started, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let running = runtime.scheduler().spawn_blocking(move || {
            started.send(()).unwrap();
            released.recv_timeout(TEST_TIMEOUT).unwrap();
            assert!(!panics, "recorded blocking panic");
        });
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        let queued = stops.then(|| runtime.scheduler().spawn_blocking(|| panic!("canceled function must not run")));
        seismograph::recorder(Configuration {
            runtime_tasks: Policy::all(false),
            ..Configuration::default()
        });
        if stops {
            control.request_stop();
        }
        release.send(()).unwrap();
        let outcome = running.join();
        if panics {
            assert!(outcome.unwrap_err().is_panic());
        } else {
            outcome.unwrap();
        }
        if let Some(queued) = queued {
            assert!(queued.join().unwrap_err().is_shutdown());
        }
        runtime.stop().unwrap();
        let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        let tasks = snapshot
            .events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    EventKind::TaskSpawned
                        | EventKind::TaskPollStarted
                        | EventKind::TaskPollFinished
                        | EventKind::TaskCompleted
                        | EventKind::TaskPanicked
                        | EventKind::TaskCanceled
                ) && event.runtime().is_some_and(|payload| payload.runtime_id == id)
            })
            .collect::<Vec<_>>();
        let outcomes = tasks.iter().map(|event| event.kind).collect::<Vec<_>>();
        let expected = if panics {
            vec![EventKind::TaskPanicked]
        } else if stops {
            vec![EventKind::TaskCompleted, EventKind::TaskCanceled]
        } else {
            vec![EventKind::TaskCompleted]
        };
        assert_eq!(outcomes, expected);
        let identities = tasks.iter().map(|event| event.runtime().unwrap().subject_id).collect::<Vec<_>>();
        assert!(identities.iter().all(|id| *id != 0));
        if stops {
            assert_ne!(identities[0], identities[1]);
        }
    }
}

#[test]
fn recording_enabled_during_an_async_poll_reports_real_outcomes() {
    let _recording = RecorderGuard::new();
    for (panics, stops) in [(false, false), (true, false), (false, true)] {
        seismograph::recorder(Configuration::default());
        let (io, _) = MonitorIo::new_mocked();
        let runtime = runtime("late-async-recording", &io);
        let id = runtime.diagnostics().unwrap().id();
        let control = support::runtime_control(&runtime);
        let (started, ready) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let running = runtime.scheduler().spawn_anywhere(
            Unaware((started, released, panics, stops)),
            |_, Unaware((started, released, panics, stops))| async move {
                started.send(()).unwrap();
                released.recv_timeout(TEST_TIMEOUT).unwrap();
                if stops {
                    pending::<()>().await;
                }
                assert!(!panics, "recorded async panic");
            },
        );
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
        seismograph::recorder(Configuration {
            runtime_tasks: Policy::all(false),
            ..Configuration::default()
        });
        if stops {
            control.request_stop();
        }
        release.send(()).unwrap();
        let outcome = running.join();
        let expected = if panics {
            assert!(outcome.unwrap_err().is_panic());
            EventKind::TaskPanicked
        } else if stops {
            assert!(outcome.unwrap_err().is_shutdown());
            EventKind::TaskCanceled
        } else {
            outcome.unwrap();
            EventKind::TaskCompleted
        };
        runtime.stop().unwrap();
        let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
        let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        let tasks = snapshot
            .events
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    EventKind::TaskSpawned
                        | EventKind::TaskPollStarted
                        | EventKind::TaskPollFinished
                        | EventKind::TaskCompleted
                        | EventKind::TaskPanicked
                        | EventKind::TaskCanceled
                ) && event.runtime().is_some_and(|payload| payload.runtime_id == id)
            })
            .collect::<Vec<_>>();
        assert_eq!(tasks.iter().map(|event| event.kind).collect::<Vec<_>>(), [expected]);
        assert_ne!(tasks[0].runtime().unwrap().subject_id, 0);
    }
}

#[test]
fn recording_reports_real_async_and_blocking_outcomes() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("recorded-outcomes", &io);
    let id = runtime.diagnostics().unwrap().id();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let completed = scheduler.spawn(async |_| 42);
    assert_eq!(completed.join().unwrap(), 42);
    assert!(
        scheduler
            .spawn(async |_| panic!("recorded async panic"))
            .join()
            .unwrap_err()
            .is_panic()
    );
    assert_eq!(scheduler.spawn_blocking(|| 7).join().unwrap(), 7);
    assert!(
        scheduler
            .spawn_blocking(|| panic!("recorded blocking panic"))
            .join()
            .unwrap_err()
            .is_panic()
    );
    let (started, ready) = mpsc::channel();
    let canceled = scheduler.spawn(move |_| async move {
        started.send(()).unwrap();
        pending::<()>().await;
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();

    runtime.stop().unwrap();
    assert!(canceled.join().unwrap_err().is_shutdown());
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let outcomes = snapshot
        .events
        .events
        .iter()
        .filter(|event| event.runtime().is_some_and(|event| event.runtime_id == id))
        .filter_map(|event| match event.kind {
            EventKind::TaskCompleted | EventKind::TaskPanicked | EventKind::TaskCanceled => Some(event.kind),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut starts = Vec::new();
    let mut finishes = Vec::new();
    for event in &snapshot.events.events {
        if let Some(payload) = event.runtime()
            && payload.runtime_id == id
        {
            match event.kind {
                EventKind::TaskPollStarted => starts.push(payload.subject_id),
                EventKind::TaskPollFinished => finishes.push(payload.subject_id),
                _ => {}
            }
        }
    }
    starts.sort_unstable();
    finishes.sort_unstable();
    assert_eq!(
        (
            outcomes.iter().filter(|kind| **kind == EventKind::TaskCompleted).count(),
            outcomes.iter().filter(|kind| **kind == EventKind::TaskPanicked).count(),
            outcomes.iter().filter(|kind| **kind == EventKind::TaskCanceled).count(),
            starts.len(),
            starts == finishes,
        ),
        (3, 2, 1, 6, true)
    );
}

#[test]
fn native_tasks_reuse_their_recorded_spawn_timestamp_and_identity() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("coherent-spawn-metadata", &io);
    let (started, ready) = mpsc::channel();
    let asynchronous = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started.clone()), |_, Unaware(started)| async move {
            started.send(()).unwrap();
            pending::<()>().await;
        });
    let (release, released) = mpsc::channel();
    let blocking = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        released.recv().unwrap();
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    ready.recv_timeout(TEST_TIMEOUT).unwrap();

    let native = capture(&runtime);
    release.send(()).unwrap();
    blocking.join().unwrap();
    runtime.stop().unwrap();
    assert!(asynchronous.join().unwrap_err().is_shutdown());
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let recorded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();

    assert_eq!(native.tasks.len(), 2);
    for task in &native.tasks {
        let spawn = recorded
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
        let payload = spawn.runtime().unwrap();
        assert_eq!(
            (task.spawned_at, task.type_descriptor.get(), task.future_size_bytes),
            (Some(spawn.timestamp), payload.value_0, Some(payload.value_1 - 1))
        );
    }
}

struct PendingBody<const N: usize> {
    started: Option<mpsc::Sender<()>>,
    release: Option<futures::channel::oneshot::Receiver<()>>,
    _payload: [u8; N],
}

impl<const N: usize> PendingBody<N> {
    fn new(started: mpsc::Sender<()>) -> Self {
        Self {
            started: Some(started),
            release: None,
            _payload: [0; N],
        }
    }
}

impl<const N: usize> Future for PendingBody<N> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if let Some(started) = self.started.take() {
            started.send(()).unwrap();
        }
        self.release
            .as_mut()
            .map_or(Poll::Pending, |release| Pin::new(release).poll(cx).map(Result::unwrap))
    }
}

#[test]
fn async_task_sizes_precede_wrapping_and_erasure() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("original-body-sizes", &io);
    let (scheduler, destination) = runtime
        .scheduler()
        .block_on(async |cx| (cx.scheduler().clone(), cx.thread().clone()))
        .unwrap();
    let (started, ready) = mpsc::channel();
    let anywhere = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started.clone()), |_, Unaware(started)| PendingBody::<64>::new(started));
    let at = scheduler.spawn_at(&destination, Unaware(started.clone()), |Unaware(started)| {
        PendingBody::<128>::new(started)
    });
    let (release, released) = futures::channel::oneshot::channel();
    let native = std::thread::scope(|scope| {
        let borrowed = &started;
        let runtime = &runtime;
        let blocked = scope.spawn(move || {
            runtime.scheduler().block_on(|_| {
                let mut body = PendingBody::<256>::new(borrowed.clone());
                body.release = Some(released);
                body
            })
        });
        for _ in 0..3 {
            ready.recv_timeout(TEST_TIMEOUT).unwrap();
        }
        let native = capture(runtime);
        release.send(()).unwrap();
        blocked.join().unwrap().unwrap();
        native
    });
    runtime.stop().unwrap();
    assert!(anywhere.join().unwrap_err().is_shutdown());
    assert!(at.join().unwrap_err().is_shutdown());
    let mut sizes = native.tasks.iter().map(|task| task.future_size_bytes.unwrap()).collect::<Vec<_>>();
    sizes.sort_unstable();
    assert_eq!(
        sizes,
        [
            u64::try_from(size_of::<PendingBody<64>>()).unwrap(),
            u64::try_from(size_of::<PendingBody<128>>()).unwrap(),
            u64::try_from(size_of::<PendingBody<256>>()).unwrap(),
        ]
    );
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let recorded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    for task in &native.tasks {
        let spawn = recorded
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
        assert_eq!(task.future_size_bytes, Some(spawn.runtime().unwrap().value_1 - 1));
    }
}

#[test]
fn queued_remote_children_retain_their_recorded_parent() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .monitor(MonitorPolicy::enabled().name("remote-parent-links").io(io))
        .build()
        .unwrap();
    let (started, ready) = mpsc::channel();
    let parent = runtime
        .scheduler()
        .spawn_anywhere(Unaware(started), |cx, Unaware(started)| async move {
            let destination = cx
                .control()
                .workers()
                .into_iter()
                .find(|worker| *worker != cx.thread())
                .unwrap()
                .clone();
            let at = cx.scheduler().spawn_at(&destination, Unaware(started.clone()), |Unaware(started)| {
                PendingBody::<64>::new(started)
            });
            let anywhere = cx
                .scheduler()
                .spawn_anywhere(Unaware(started), |Unaware(started)| PendingBody::<128>::new(started));
            at.await.unwrap();
            anywhere.await.unwrap();
        });
    for _ in 0..2 {
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
    }
    let native = capture(&runtime);
    runtime.stop().unwrap();
    assert!(parent.join().unwrap_err().is_shutdown());
    assert_eq!(native.tasks.len(), 3);
    let root = native.tasks.iter().find(|task| task.parent.is_none()).unwrap();
    let children = native.tasks.iter().filter(|task| task.id != root.id).collect::<Vec<_>>();
    assert!(children.iter().all(|task| task.parent == Some(root.id)));
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let recorded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    for child in children {
        let spawn = recorded
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
        assert_eq!(spawn.runtime().unwrap().related_id, root.id.get());
    }
}

#[test]
fn blocking_children_retain_their_recorded_parent() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("blocking-parent-links", &io);
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (started, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let parent = runtime.scheduler().spawn_blocking(move || {
        let child = scheduler.spawn_blocking(|| 42);
        started.send(child).unwrap();
        released.recv_timeout(TEST_TIMEOUT).unwrap();
        panic!("recorded parent panic");
    });
    let child = ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let native = capture(&runtime);
    release.send(()).unwrap();
    assert!(parent.join().unwrap_err().is_panic());
    assert_eq!(child.join().unwrap(), 42);
    runtime.stop().unwrap();
    assert_eq!(native.tasks.len(), 2);
    let parent = native
        .tasks
        .iter()
        .find(|task| task.activity.unwrap().state == TaskActivityState::Running)
        .unwrap();
    let child = native
        .tasks
        .iter()
        .find(|task| task.activity.unwrap().state == TaskActivityState::Ready)
        .unwrap();
    assert_eq!(child.parent, Some(parent.id));
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let recorded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let spawn = recorded
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
    assert_eq!(spawn.runtime().unwrap().related_id, parent.id.get());
}

#[test]
fn collection_reports_the_native_task_limit_instead_of_claiming_completeness() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("bounded-tasks", &io);
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    for _ in 0..260 {
        drop(scheduler.spawn(async |_| pending::<()>().await));
    }
    let (started, ready) = mpsc::channel();
    drop(scheduler.spawn(move |_| async move {
        started.send(()).unwrap();
        pending::<()>().await;
    }));
    ready.recv_timeout(TEST_TIMEOUT).unwrap();

    let snapshot = capture(&runtime);
    runtime.stop().unwrap();
    let core = snapshot.workers.iter().find(|worker| worker.role == WorkerRole::Core).unwrap();

    assert_eq!((snapshot.tasks.len(), core.task_collection), (256, TaskCollection::Truncated));
}

#[test]
fn a_blocked_async_worker_times_out_and_can_answer_a_later_request() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("blocked-worker", &io);
    let (started, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware((started, released)), |_, Unaware((started, released))| async move {
            started.send(()).unwrap();
            released.recv_timeout(TEST_TIMEOUT).unwrap();
        });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();

    let blocked = runtime
        .diagnostics()
        .unwrap()
        .capture_with_timeout(CaptureOptions::default())
        .unwrap();
    release.send(()).unwrap();
    task.join().unwrap();
    // Completion precedes the next command batch, which retires the canceled request.
    runtime.scheduler().block_on(async |_| ()).unwrap();
    let recovered = capture(&runtime);
    runtime.stop().unwrap();
    let core_status = |snapshot: &RuntimeSnapshot| {
        snapshot
            .workers
            .iter()
            .find(|worker| worker.role == WorkerRole::Core)
            .unwrap()
            .task_collection
    };

    assert_eq!(
        (core_status(&blocked), core_status(&recovered)),
        (TaskCollection::TimedOut, TaskCollection::Complete)
    );
}

#[test]
fn stopping_one_runtime_retires_only_its_endpoint_and_workers() {
    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let first = runtime("endpoint-one", &io);
    let second = runtime("endpoint-two", &io);
    assert_eq!(names(&io), ["endpoint-one", "endpoint-two"]);

    first.stop().unwrap();
    let first = runtimes().into_iter().find(|runtime| runtime.name == "endpoint-one").unwrap();
    let second_snapshot = capture(&second);
    let remaining = names(&io);
    second.stop().unwrap();

    assert_eq!(
        (first.state, first.tasks.is_empty(), second_snapshot.state, remaining, names(&io)),
        (
            RuntimeState::Stopped,
            true,
            RuntimeState::Running,
            vec!["endpoint-two".to_owned()],
            Vec::new()
        )
    );
}

#[test]
fn monitor_startup_failure_is_returned_without_publishing_an_endpoint() {
    let _recording = RecorderGuard::new();
    let (io, control) = MonitorIo::new_mocked();
    control.fail_next(IoOperation::Bind, std::io::ErrorKind::PermissionDenied.into());

    let result = Runtime::builder()
        .monitor(MonitorPolicy::enabled().name("failed-monitor").io(io.clone()))
        .build();

    drop(result.unwrap_err());
    assert_eq!(names(&io), Vec::<String>::new());
}

#[test]
fn stopping_reports_a_monitor_failure_after_removing_the_endpoint() {
    let _recording = RecorderGuard::new();
    let (io, control) = MonitorIo::new_mocked();
    control.fail_next(
        IoOperation::Accept,
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected monitor accept failure"),
    );
    let runtime = runtime("failed-listener", &io);
    let descriptor = Directory::current(&io).unwrap().descriptors().unwrap().next().unwrap().unwrap();
    let mut connection = Connection::connect(&io, &descriptor, TEST_TIMEOUT).unwrap();
    connection.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();
    write_request(&mut connection, 1, &Request::Hello).unwrap();
    assert!(matches!(read_response(&mut connection).unwrap(), (1, Response::Hello { .. })));

    let error = runtime.stop().unwrap_err();

    assert_eq!(
        (error.to_string().contains("injected monitor accept failure"), names(&io)),
        (true, Vec::new())
    );
}

#[test]
fn shutdown_records_cancellation_for_a_queued_blocking_function() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("canceled-blocking", &io);
    let control = support::runtime_control(&runtime);
    let (started, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let running = runtime.scheduler().spawn_blocking(move || {
        started.send(()).unwrap();
        released.recv_timeout(TEST_TIMEOUT).unwrap();
    });
    ready.recv_timeout(TEST_TIMEOUT).unwrap();
    let queued = runtime.scheduler().spawn_blocking(|| panic!("a canceled function must not run"));
    let snapshot = capture(&runtime);
    let id = snapshot
        .tasks
        .iter()
        .find(|task| task.activity.unwrap().state == TaskActivityState::Ready)
        .unwrap()
        .id;

    control.request_stop();
    release.send(()).unwrap();
    runtime.stop().unwrap();
    running.join().unwrap();
    let canceled = queued.join().unwrap_err();
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let events = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let outcomes = events
        .events
        .events
        .iter()
        .filter_map(|event| {
            event
                .runtime()
                .filter(|payload| {
                    payload.runtime_id == snapshot.id
                        && payload.subject_id == id.get()
                        && matches!(
                            event.kind,
                            EventKind::TaskCompleted
                                | EventKind::TaskPanicked
                                | EventKind::TaskCanceled
                                | EventKind::TaskPollStarted
                                | EventKind::TaskPollFinished
                        )
                })
                .map(|_| event.kind)
        })
        .collect::<Vec<_>>();

    assert_eq!((canceled.is_shutdown(), outcomes), (true, vec![EventKind::TaskCanceled]));
}

#[test]
fn the_monitor_connection_captures_runtime_diagnostics_and_preserves_recording_configuration() {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("connected-monitor", &io);
    let id = runtime.diagnostics().unwrap().id();
    let descriptor = Directory::current(&io).unwrap().descriptors().unwrap().next().unwrap().unwrap();
    let mut connection = Connection::connect(&io, &descriptor, TEST_TIMEOUT).unwrap();
    connection.set_read_timeout(Some(TEST_TIMEOUT)).unwrap();

    write_request(&mut connection, 1, &Request::Hello).unwrap();
    let hello = read_response(&mut connection).unwrap();
    write_request(
        &mut connection,
        2,
        &Request::CaptureSnapshot(seismograph_protocol::message::SnapshotOptions::default()),
    )
    .unwrap();
    let (request, Response::Snapshot(bytes)) = read_response(&mut connection).unwrap() else {
        panic!("the monitor must return the requested snapshot");
    };
    let snapshot = seismograph::snapshot::decode(&bytes).unwrap();
    let source = snapshot
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .unwrap();
    let snapshot = seismograph_runtime::snapshot::decode(&source.data).unwrap();
    let captured = snapshot
        .runtimes
        .iter()
        .find(|runtime| runtime.name == "connected-monitor")
        .unwrap();
    runtime.stop().unwrap();

    assert_eq!(
        (
            hello,
            request,
            (captured.id, captured.state),
            seismograph::recorder::is_enabled(EventClass::General),
            seismograph::recorder::is_enabled(EventClass::RuntimeTask),
            names(&io),
        ),
        (
            (
                1,
                Response::Hello {
                    instance_id: descriptor.instance_id,
                    recording: RecordingConfiguration {
                        general_events: RecordingPolicy {
                            enabled: true,
                            ..RecordingPolicy::default()
                        },
                        ..RecordingConfiguration::default()
                    },
                },
            ),
            2,
            (id, RuntimeState::Running),
            true,
            false,
            Vec::new(),
        )
    );
}

#[test]
fn task_sizes_describe_user_futures_before_runtime_wrapping() {
    struct Body {
        started: Option<mpsc::Sender<()>>,
        _data: [u8; 37],
    }

    impl Future for Body {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            if let Some(started) = self.get_mut().started.take() {
                started.send(()).unwrap();
            }
            Poll::Pending
        }
    }

    let _recording = RecorderGuard::new();
    let (io, _) = MonitorIo::new_mocked();
    let runtime = runtime("user-bodies", &io);
    let (started, ready) = mpsc::channel();
    drop(
        runtime
            .scheduler()
            .spawn_anywhere(Unaware(started.clone()), |_, Unaware(started)| Body {
                started: Some(started),
                _data: [0; 37],
            }),
    );
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    drop(scheduler.spawn_anywhere(Unaware(started.clone()), |Unaware(started)| Body {
        started: Some(started),
        _data: [0; 37],
    }));
    runtime
        .scheduler()
        .block_on(async move |cx| {
            drop(cx.scheduler().spawn(move |_| Body {
                started: Some(started),
                _data: [0; 37],
            }));
        })
        .unwrap();
    for _ in 0..3 {
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
    }

    let snapshot = capture(&runtime);
    runtime.stop().unwrap();
    let sizes = snapshot.tasks.iter().map(|task| task.future_size_bytes).collect::<Vec<_>>();

    assert_eq!(sizes, vec![Some(size_of::<Body>() as u64); 3]);
}

fn growing_pool(name: &str, policy: BlockingPoolPolicy, expected_blocking_workers: usize) {
    let _recording = RecorderGuard::new();
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });
    let (io, _) = MonitorIo::new_mocked();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(2))
        .blocking_pool(policy.max(2))
        .monitor(MonitorPolicy::enabled().name(name).io(io))
        .build()
        .unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, ready) = mpsc::channel();
    let tasks = (0..6)
        .map(|_| {
            let release = Arc::clone(&release);
            let started = started.clone();
            scheduler.spawn_blocking(move || {
                started.send(()).unwrap();
                let (released, changed) = &*release;
                let mut released = released.lock().unwrap();
                while !*released {
                    let waited = changed.wait_timeout(released, TEST_TIMEOUT).unwrap();
                    assert!(!waited.1.timed_out(), "the test must release its blocking functions");
                    released = waited.0;
                }
            })
        })
        .collect::<Vec<_>>();
    for _ in 0..2 {
        ready.recv_timeout(TEST_TIMEOUT).unwrap();
    }

    let snapshot = capture(&runtime);
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    for task in tasks {
        task.join().unwrap();
    }
    runtime.stop().unwrap();
    let bytes = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let events = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let mut matched_workers = Vec::new();
    for task in &snapshot.tasks {
        let worker_for = |kind| {
            events
                .events
                .events
                .iter()
                .find_map(|event| {
                    event
                        .runtime()
                        .filter(|payload| event.kind == kind && payload.runtime_id == snapshot.id && payload.subject_id == task.id.get())
                        .and_then(|payload| payload.worker_id)
                })
                .unwrap()
        };
        matched_workers.push(worker_for(EventKind::TaskPollStarted) == worker_for(EventKind::TaskCompleted));
    }
    let running = snapshot
        .tasks
        .iter()
        .filter(|task| task.activity.unwrap().state == TaskActivityState::Running)
        .count();
    let queued = snapshot
        .tasks
        .iter()
        .filter(|task| task.activity.unwrap().state == TaskActivityState::Ready)
        .count();
    let workers = snapshot.workers.iter().filter(|worker| worker.role == WorkerRole::Blocking).count();

    assert_eq!(
        (workers, running, queued, matched_workers),
        (expected_blocking_workers, 2, 4, vec![true; 6])
    );
}

#[test]
fn a_shared_pool_registers_new_threads_and_records_outcomes_on_the_execution_worker() {
    growing_pool("growing-shared-pool", BlockingPoolPolicy::shared(), 2);
}

#[test]
fn a_per_worker_pool_registers_new_threads_without_merging_other_pools() {
    growing_pool("growing-per-worker-pool", BlockingPoolPolicy::per_worker(), 3);
}
