// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates asynchronous and blocking task lifecycle instrumentation.

use std::collections::VecDeque;

use seismograph::recorder::{Configuration, Policy};
use seismograph_runtime::{Runtime, TaskIdentity, TaskOutcome, WorkerMetadata, WorkerRole};

fn main() {
    seismograph::recorder(Configuration {
        runtime_tasks: Policy::all(false),
        ..Configuration::default()
    });

    let runtime = Runtime::builder("task-flow").configured_workers(2).build();
    let async_worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
    let blocking_worker = runtime.blocking_worker(WorkerMetadata::new(WorkerRole::Blocking));

    let mut async_queue = VecDeque::new();
    let async_identity = TaskIdentity::from_location(std::panic::Location::caller());
    let task = runtime.async_task(async_identity, size_of::<u64>());
    let queued = task
        .expect("runtime task recording is enabled")
        .with_context(Some(async_worker.origin()), None)
        .enqueued(async_queue.len() as u64);
    async_queue.push_back(queued);

    let queued = async_queue.pop_front().expect("the task was queued above");
    let task = queued.activated(&async_worker);
    let poll = task.poll_started(&async_worker);
    let task = poll.finished(None).expect("a pending poll returns its task");
    let poll = task.poll_started(&async_worker);
    assert!(poll.finished(Some(TaskOutcome::Completed)).is_none());

    let mut blocking_queue = VecDeque::new();
    let blocking_identity = TaskIdentity::from_location(std::panic::Location::caller());
    let task = runtime.blocking_task(blocking_identity, size_of::<u64>());
    let queued = task
        .expect("runtime task recording is enabled")
        .with_context(Some(async_worker.origin()), None)
        .enqueued(blocking_queue.len() as u64);
    blocking_queue.push_back(queued);

    let queued = blocking_queue.pop_front().expect("the task was queued above");
    let execution = queued.started(&blocking_worker);
    execution.finished(TaskOutcome::Completed);
}
