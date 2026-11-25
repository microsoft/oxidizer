// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Monitoring behavior for task submissions rejected during registration.

use std::cell::Cell;
use std::future::Pending;
use std::rc::Rc;
use std::sync::Mutex;

use arty_executor::{Executor, Shutdown, ShutdownCause, TaskCompletion, TaskMetadata, TaskObserver, TaskPanicPayload};
use seismograph::recorder::event::EventKind;
use seismograph_runtime::worker::WorkerHandle;

static RECORDING: Mutex<()> = Mutex::new(());

#[derive(Debug)]
struct Completion;

impl TaskCompletion<()> for Completion {
    fn completed(self, (): ()) {}

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        arty_executor::discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

#[derive(Debug)]
struct CaptureCancellation(Rc<Cell<bool>>);

impl TaskCompletion<()> for CaptureCancellation {
    #[expect(clippy::panic, reason = "this callback proves a rejected task cannot complete")]
    fn completed(self, (): ()) {
        panic!("rejected task must not complete");
    }

    #[expect(clippy::panic, reason = "this callback proves a rejected task cannot report a task panic")]
    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        arty_executor::discard_task_panic_payload(panic);
        panic!("rejected task must not report a task panic");
    }

    fn cancelled(self, _cause: ShutdownCause) {
        self.0.set(true);
    }
}

enum Submission {
    Stop(Shutdown),
    Panic,
}

impl IntoFuture for Submission {
    type Output = ();
    type IntoFuture = Pending<()>;

    fn into_future(self) -> Self::IntoFuture {
        match self {
            Self::Stop(shutdown) => shutdown.trigger(ShutdownCause::default()),
            Self::Panic => std::panic::resume_unwind(Box::new("conversion panic")),
        }
        std::future::pending()
    }
}

struct ResetRecorder;

impl Drop for ResetRecorder {
    fn drop(&mut self) {
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }
}

struct PanicDropObserver;

impl TaskObserver for PanicDropObserver {}

struct PanicOnDrop;

impl Drop for PanicOnDrop {
    #[expect(clippy::panic, reason = "the test needs a panic payload whose destructor panics")]
    fn drop(&mut self) {
        panic!("panic payload drop panic");
    }
}

impl Drop for PanicDropObserver {
    fn drop(&mut self) {
        std::panic::resume_unwind(Box::new(PanicOnDrop));
    }
}

fn task_metadata<T: 'static>(worker: &WorkerHandle) -> TaskMetadata {
    TaskMetadata::capture::<T>(worker.task_events(), None, size_of::<T>())
}

#[test]
fn rejected_or_panicking_conversion_emits_no_task_lifecycle_events() {
    let _recording = RECORDING.lock().unwrap();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });

    for (name, stop_before_conversion, submission) in [
        ("rejected-before-conversion", true, None),
        ("rejected-during-conversion", false, None),
        ("panicking-conversion", false, Some(Submission::Panic)),
    ] {
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new(name, 1));
        let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
            seismograph_runtime::worker::WorkerRole::Core,
        ));
        let executor = Executor::builder().monitor(worker.handle()).build();
        if stop_before_conversion {
            executor.shutdown().trigger(ShutdownCause::default());
        }

        let submission = submission.unwrap_or_else(|| Submission::Stop(executor.shutdown()));
        let register = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor.tasks().task(submission).monitor(None).register(Completion);
        }));

        assert_eq!(register.is_err(), name == "panicking-conversion");

        let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        assert!(!snapshot.events.events.iter().any(|event| {
            matches!(event.kind, EventKind::TaskSpawned | EventKind::TaskCanceled)
                && event.runtime().is_some_and(|payload| payload.runtime_id == runtime.id())
        }));
    }
}

#[test]
fn unmonitored_shutdown_rejection_discards_recorded_metadata_without_panicking() {
    let _recording = RECORDING.lock().unwrap();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });
    let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("unmonitored-rejection", 1));
    let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
        seismograph_runtime::worker::WorkerRole::Core,
    ));
    let executor = Executor::builder().build();
    executor.shutdown().trigger(ShutdownCause::default());
    let canceled = Rc::new(Cell::new(false));

    executor
        .tasks()
        .task(std::future::pending::<()>())
        .monitor(Some(task_metadata::<()>(&worker.handle())))
        .register(CaptureCancellation(Rc::clone(&canceled)));

    assert!(canceled.get());
}

#[test]
fn monitored_rejections_report_cancellation_and_contain_observer_drop_panics() {
    let _recording = RECORDING.lock().unwrap();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });

    for stop_before_conversion in [true, false] {
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("monitored-rejection", 1));
        let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
            seismograph_runtime::worker::WorkerRole::Core,
        ));
        let executor = Executor::builder().monitor(worker.handle()).build();
        if stop_before_conversion {
            executor.shutdown().trigger(ShutdownCause::default());
        }
        let canceled = Rc::new(Cell::new(false));
        let metadata = task_metadata::<()>(&worker.handle());
        let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        let task = snapshot
            .events
            .events
            .iter()
            .find_map(|event| {
                (event.kind == EventKind::TaskSpawned)
                    .then(|| event.runtime())
                    .flatten()
                    .filter(|payload| payload.runtime_id == runtime.id())
                    .map(|payload| payload.subject_id)
            })
            .unwrap();
        let submission = Submission::Stop(executor.shutdown());

        let register = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            executor
                .tasks()
                .task(submission)
                .observe(PanicDropObserver)
                .monitor(Some(metadata))
                .register(CaptureCancellation(Rc::clone(&canceled)));
        }));

        register.unwrap();
        assert!(canceled.get());
        let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
        assert!(
            snapshot.events.events.iter().any(|event| {
                event.kind == EventKind::TaskCanceled && event.runtime().is_some_and(|payload| payload.subject_id == task)
            })
        );
    }
}

#[test]
fn panicking_task_emits_one_terminal_event() {
    let _recording = RECORDING.lock().unwrap();
    let _reset = ResetRecorder;
    seismograph::recorder(seismograph::recorder::Configuration {
        runtime_tasks: seismograph::recorder::Policy::all(false),
        ..seismograph::recorder::Configuration::default()
    });
    let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("panicking-task", 1));
    let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
        seismograph_runtime::worker::WorkerRole::Core,
    ));
    let mut executor = Executor::builder().monitor(worker.handle()).build();
    executor
        .tasks()
        .task(async { panic!("task panic") })
        .monitor(Some(task_metadata::<()>(&worker.handle())))
        .register(Completion);

    let _ = executor.execute_cycle();

    let bytes = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let snapshot = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    assert_eq!(
        snapshot
            .events
            .events
            .iter()
            .filter(|event| {
                event.kind == EventKind::TaskPanicked && event.runtime().is_some_and(|payload| payload.runtime_id == runtime.id())
            })
            .count(),
        1
    );
}
