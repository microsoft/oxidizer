// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native Seismograph task monitoring contracts.

use std::sync::mpsc;

use arty_executor::{Executor, TaskCompletion};
use seismograph_runtime::snapshot::{CaptureOptions, TaskActivityState, TaskCollection};
use seismograph_runtime::{Runtime, TaskIdentity, WorkerMetadata, WorkerRole};

#[derive(Debug)]
struct Discard;

impl TaskCompletion<()> for Discard {
    fn completed(self, (): ()) {}

    fn panicked(self, panic: arty_executor::TaskPanicPayload, _cause: Option<arty_executor::ShutdownCause>) {
        arty_executor::discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: arty_executor::ShutdownCause) {}
}

#[test]
fn executor_collects_metadata_from_its_queued_task() {
    let runtime = Runtime::builder("executor-native-monitoring").configured_workers(1).build();
    let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    let (requests, pending_requests) = mpsc::channel();
    worker
        .register_task_provider(move |request| requests.send(request).unwrap())
        .unwrap();

    let mut executor = Executor::builder().monitoring(worker).build();
    let parent = TaskIdentity::from_location(std::panic::Location::caller());
    let identity = TaskIdentity::from_location(std::panic::Location::caller());
    let queued = runtime.async_task(None, identity, Some(parent.id()), 37).enqueued(0);
    executor
        .tasks()
        .task(identity, std::future::pending::<()>())
        .monitor(queued)
        .register(Discard);

    let capture_runtime = runtime.clone();
    let capture = std::thread::spawn(move || capture_runtime.capture(CaptureOptions::default()).unwrap());
    executor.collect(pending_requests.recv().unwrap());
    let snapshot = capture.join().unwrap();

    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(
        (
            snapshot.tasks[0].id,
            snapshot.tasks[0].parent,
            snapshot.tasks[0].future_size_bytes,
            snapshot.tasks[0].activity.unwrap().state,
            snapshot.workers[0].task_collection,
        ),
        (
            identity.id(),
            Some(parent.id()),
            Some(37),
            TaskActivityState::Ready,
            TaskCollection::Complete,
        )
    );
}
