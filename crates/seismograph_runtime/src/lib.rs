// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![forbid(unsafe_code)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Runtime diagnostics and task lifecycle reporting for [`seismograph`].
//!
//! [`RuntimeBuilder`] creates a [`Runtime`]. The runtime owns worker
//! registrations and creates dedicated [`asynchronous::Worker`] and
//! [`blocking::Worker`] handles.
//!
//! Both task types use the shared `Task*` event kinds. Task methods return
//! lightweight handles when runtime task recording is enabled. The caller
//! records queue insertion and moves each token through execution, poll, and
//! outcome transitions. Dropping an unfinished queued token records cancellation.
//!
//! [`TaskIdentity`] lets the runtime assign an identity before it records the
//! task. Event recording retains only events emitted after recording starts.
//! [`model::RuntimeModel`] reconstructs runtime, worker, and task state from the
//! retained event ring. A task that predates recording appears only after it
//! emits a retained event, such as a later poll.
//!
//! # Runtime integration
//!
//! Create one [`Runtime`] during runtime startup. Keep it until all runtime
//! workers stop. Create it even when the application does not start a
//! Seismograph monitor connection.
//!
//! Create one [`asynchronous::Worker`] on each asynchronous worker thread.
//! Create one [`blocking::Worker`] on each blocking worker thread. Store each
//! worker in thread-local runtime state and borrow it for lifecycle callbacks.
//! Do not clone a worker for each callback.
//!
//! When an asynchronous task is spawned, call [`Runtime::async_task`] with its
//! origin worker, identity, parent, and concrete body size. The method returns
//! `None` when runtime-task recording is disabled. If it returns a handle, move
//! the handle with the task and call [`asynchronous::TaskHandle::enqueued`] when
//! the task enters the executor queue.
//!
//! Pass the resulting optional [`asynchronous::QueuedTask`] into the executor.
//! On the first poll, call [`asynchronous::QueuedTask::activated`] and then
//! [`asynchronous::Task::poll_started`]. After every poll, call
//! [`asynchronous::Poll::finished`] with `None` for a pending poll or with the
//! terminal outcome.
//!
//! If a task has no queued handle, keep a dormant executor state. When a later
//! poll starts, create an active token with [`asynchronous::Task::unrecorded`].
//! This records the new poll without inventing queue or activation events.
//! Read [`asynchronous::Worker::is_recording`] once at the start of an executor
//! cycle. Select a recorded or silent poll loop for the full cycle. The silent
//! loop must transform task state without recorder checks or clock reads.
//!
//! For blocking work, call [`Runtime::blocking_task`] at submission. If it
//! returns a handle, call [`blocking::TaskHandle::enqueued`] while the queue
//! lock supplies the queue length. Call [`blocking::QueuedTask::started`] when
//! a worker removes the job, and call [`blocking::Execution::finished`] after
//! the function returns or panics.
//!
//! Call [`asynchronous::Worker::parked`] before an asynchronous worker waits,
//! and call [`asynchronous::Worker::unparked`] after it wakes. Apply the same
//! rule to blocking workers. Call [`Runtime::stopping`] when shutdown starts,
//! and call [`Runtime::stop`] after all workers stop.
//!
//! Keep the disabled path short. Runtime and worker handles are always present,
//! but task handles are optional. Do not add a task registry, provider callback,
//! mutex, clock read, or shared-reference clone to the disabled task path.
//!
//! # Example
//!
//! ```
//! use seismograph_runtime::{Runtime, TaskIdentity, TaskOutcome, WorkerMetadata, WorkerRole};
//!
//! seismograph::recorder(seismograph::recorder::Configuration {
//!     runtime_tasks: seismograph::recorder::Policy::all(false),
//!     ..seismograph::recorder::Configuration::default()
//! });
//! let runtime = Runtime::builder("primary").configured_workers(1).build();
//! let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
//! let identity = TaskIdentity::from_location(std::panic::Location::caller());
//! let queued = runtime.async_task(identity, 0).unwrap().enqueued(0);
//! let task = queued.activated(&worker);
//! let poll = task.poll_started(&worker);
//! assert!(
//!     poll.finished(&worker, Some(TaskOutcome::Completed))
//!         .is_none()
//! );
//! ```

pub mod asynchronous;
pub mod blocking;
pub mod model;
pub mod snapshot;

mod runtime;
mod task_identity;
mod task_outcome;
mod worker;

pub use runtime::{Runtime, RuntimeBuilder};
pub use task_identity::TaskIdentity;
pub use task_outcome::TaskOutcome;
pub use worker::{WorkerMetadata, WorkerOrigin, WorkerRole};
