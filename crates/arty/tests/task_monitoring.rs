// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public async and blocking task monitoring contracts.

#![cfg(feature = "rt")]
#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::{Mutex, mpsc};
use std::task::{Context, Poll};

use arty::runtime::Runtime;
use arty::runtime::hook::TaskPanic;
use arty::runtime::policy::{BlockingPoolPolicy, WorkersPolicy};
use arty::task::{JoinError, JoinHandle};
use seismograph::recorder::event::{Event, EventKind};
use seismograph::recorder::runtime::TaskId;
use seismograph_runtime::WorkerRole;
use testing_aids::TEST_TIMEOUT;

static RECORDING: Mutex<()> = Mutex::new(());

trait JoinHandleExt {
    type Output;

    fn join(self) -> Result<Self::Output, JoinError>;
}

impl<R> JoinHandleExt for JoinHandle<R>
where
    R: Send + 'static,
{
    type Output = R;

    fn join(self) -> Result<Self::Output, JoinError> {
        futures::executor::block_on(self)
    }
}

struct RecordingGuard;

impl RecordingGuard {
    fn start() -> Self {
        seismograph::recorder(seismograph::recorder::Configuration {
            runtime_tasks: seismograph::recorder::Policy::all(false),
            ..seismograph::recorder::Configuration::default()
        });
        seismograph::recorder::clear_buffers().unwrap();
        Self
    }
}

impl Drop for RecordingGuard {
    fn drop(&mut self) {
        seismograph::recorder(seismograph::recorder::Configuration::default());
        seismograph::recorder::clear_buffers().unwrap();
    }
}

struct RuntimeCapture {
    events: Vec<Event>,
    workers: BTreeMap<u64, WorkerRole>,
}

fn capture_runtime(name: &str) -> RuntimeCapture {
    let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let source = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_runtime::snapshot::source::ID)
        .expect("Arty registers the runtime source");
    let source = seismograph_runtime::snapshot::decode(&source.data).unwrap();
    let runtime = source
        .runtimes
        .iter()
        .find(|runtime| runtime.name == name)
        .expect("the named Arty runtime is active");
    let runtime_id = runtime.id;
    let workers = runtime.workers.iter().map(|worker| (worker.id.get(), worker.role)).collect();
    let mut events = decoded
        .events
        .events
        .into_iter()
        .filter(|event| event.runtime().is_some_and(|payload| payload.runtime_id == runtime_id))
        .collect::<Vec<_>>();
    events.sort_unstable_by_key(|event| (event.timestamp, event.thread_id, event.sequence));
    RuntimeCapture { events, workers }
}

fn task_events(events: &[Event]) -> BTreeMap<TaskId, Vec<&Event>> {
    let mut tasks = BTreeMap::<TaskId, Vec<&Event>>::new();
    for event in events {
        if !matches!(
            event.kind,
            EventKind::TaskEnqueued
                | EventKind::TaskMaterialized
                | EventKind::TaskPollStarted
                | EventKind::TaskPollFinished
                | EventKind::TaskCompleted
                | EventKind::TaskCanceled
                | EventKind::TaskPanicked
        ) {
            continue;
        }
        let payload = event.runtime().unwrap();
        let Some(task) = TaskId::from_raw(payload.subject_id) else {
            continue;
        };
        tasks.entry(task).or_default().push(event);
    }
    tasks
}

fn assert_queue_and_execution_timing(events: &[&Event], workers: &BTreeMap<u64, WorkerRole>, role: WorkerRole) {
    let enqueued = events
        .iter()
        .find(|event| event.kind == EventKind::TaskEnqueued)
        .expect("recorded tasks retain their queue event");
    let materialized = events.iter().find(|event| event.kind == EventKind::TaskMaterialized);
    if let Some(materialized) = materialized {
        let payload = materialized.runtime().unwrap();
        assert_eq!(payload.value_0, materialized.timestamp.ticks() - enqueued.timestamp.ticks());
        assert!(payload.value_1 > 0, "materialization retains the concrete task body size");
        let worker = payload.worker_id.expect("execution identifies its worker");
        assert_eq!(workers.get(&worker.get()), Some(&role));
    }
}

fn assert_poll_timing(events: &[&Event], workers: &BTreeMap<u64, WorkerRole>) -> BTreeSet<u64> {
    let mut started = None;
    let mut outcomes = BTreeSet::new();
    for event in events {
        match event.kind {
            EventKind::TaskPollStarted => {
                let payload = event.runtime().unwrap();
                let worker = payload.worker_id.expect("poll start identifies its worker");
                assert_eq!(workers.get(&worker.get()), Some(&WorkerRole::Core));
                started = Some(event.timestamp);
            }
            EventKind::TaskPollFinished => {
                let start = started.take().expect("each poll finish follows a poll start");
                let payload = event.runtime().unwrap();
                assert_eq!(payload.value_0, event.timestamp.ticks() - start.ticks());
                outcomes.insert(payload.value_1);
            }
            _ => {}
        }
    }
    outcomes
}

struct PendingProbe(Option<mpsc::Sender<()>>);

impl Future for PendingProbe {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(started) = self.0.take() {
            started.send(()).unwrap();
        }
        Poll::Pending
    }
}

#[test]
fn async_tasks_record_queue_poll_timing_and_terminal_outcomes() {
    let _recording = RECORDING.lock().unwrap();
    let _guard = RecordingGuard::start();
    let name = "arty-async-task-monitoring";
    let runtime = Runtime::builder()
        .name(name)
        .workers(WorkersPolicy::exactly(1))
        .on(TaskPanic::ignore())
        .build()
        .unwrap();
    let control = runtime.scheduler().block_on(async |context| context.control().clone()).unwrap();
    seismograph::recorder::clear_buffers().unwrap();

    let completed = runtime
        .scheduler()
        .block_on(async |context| context.scheduler().spawn(async |_| 42))
        .unwrap();
    assert_eq!(completed.join().unwrap(), 42);
    let panicked = runtime
        .scheduler()
        .block_on(async |context| context.scheduler().spawn(async |_| panic!("recorded async panic")))
        .unwrap();
    assert!(panicked.join().unwrap_err().is_panic());
    let (started_tx, started_rx) = mpsc::channel();
    let canceled = runtime
        .scheduler()
        .block_on(async |context| context.scheduler().spawn(move |_| PendingProbe(Some(started_tx))))
        .unwrap();
    started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    control.shutdown();
    assert!(canceled.join().unwrap_err().is_shutdown());

    let capture = capture_runtime(name);
    let tasks = task_events(&capture.events);
    let mut outcomes = BTreeSet::new();
    let mut canceled_tasks = 0;
    for events in tasks.values() {
        assert_queue_and_execution_timing(events, &capture.workers, WorkerRole::Core);
        outcomes.extend(assert_poll_timing(events, &capture.workers));
        if events.iter().any(|event| event.kind == EventKind::TaskCanceled) {
            canceled_tasks += 1;
        }
        let queued = events.iter().find(|event| event.kind == EventKind::TaskEnqueued).unwrap();
        assert_eq!(queued.runtime().unwrap().value_1, 0);
    }
    assert!(outcomes.contains(&1), "a successful terminal poll is retained");
    assert!(outcomes.contains(&3), "a panicked terminal poll is retained");
    assert!(outcomes.contains(&0), "a pending poll is retained");
    assert_eq!(canceled_tasks, 1);

    runtime.stop().unwrap();
}

#[test]
fn blocking_tasks_record_queue_execution_timing_depth_and_terminal_outcomes() {
    let _recording = RECORDING.lock().unwrap();
    let _guard = RecordingGuard::start();
    let name = "arty-blocking-task-monitoring";
    let runtime = Runtime::builder()
        .name(name)
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .build()
        .unwrap();
    let control = runtime.scheduler().block_on(async |context| context.control().clone()).unwrap();
    seismograph::recorder::clear_buffers().unwrap();
    let scheduler = runtime.scheduler();

    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let blocker = scheduler.spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    });
    started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    let completed = scheduler.spawn_blocking(|| 42);
    let panicked = scheduler.spawn_blocking(|| panic!("recorded blocking panic"));
    release_tx.send(()).unwrap();
    blocker.join().unwrap();
    assert_eq!(completed.join().unwrap(), 42);
    assert!(panicked.join().unwrap_err().is_panic());

    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let blocker = scheduler.spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    });
    started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
    let canceled = scheduler.spawn_blocking(|| 7);
    control.shutdown();
    release_tx.send(()).unwrap();
    let _ = blocker.join();
    assert!(canceled.join().unwrap_err().is_shutdown());

    let capture = capture_runtime(name);
    let tasks = task_events(&capture.events);
    let queue_depths = tasks
        .values()
        .filter_map(|events| events.iter().find(|event| event.kind == EventKind::TaskEnqueued))
        .map(|event| event.runtime().unwrap().value_1)
        .collect::<Vec<_>>();
    assert_eq!(queue_depths, [0, 0, 1, 0, 0]);

    let mut completed_tasks = 0;
    let mut panicked_tasks = 0;
    let mut canceled_tasks = 0;
    for events in tasks.values() {
        assert_queue_and_execution_timing(events, &capture.workers, WorkerRole::Blocking);
        let materialized = events.iter().find(|event| event.kind == EventKind::TaskMaterialized);
        let terminal = events.iter().find(|event| {
            matches!(
                event.kind,
                EventKind::TaskCompleted | EventKind::TaskPanicked | EventKind::TaskCanceled
            )
        });
        match terminal.map(|event| event.kind) {
            Some(EventKind::TaskCompleted) => completed_tasks += 1,
            Some(EventKind::TaskPanicked) => panicked_tasks += 1,
            Some(EventKind::TaskCanceled) => {
                canceled_tasks += 1;
                assert!(materialized.is_none(), "a canceled queued task never starts execution");
            }
            _ => {}
        }
        if let (Some(materialized), Some(terminal)) = (materialized, terminal) {
            let payload = terminal.runtime().unwrap();
            assert_eq!(payload.value_0, terminal.timestamp.ticks() - materialized.timestamp.ticks());
            assert_eq!(payload.worker_id, materialized.runtime().unwrap().worker_id);
        }
    }
    assert_eq!((completed_tasks, panicked_tasks, canceled_tasks), (3, 1, 1));

    runtime.stop().unwrap();
}
