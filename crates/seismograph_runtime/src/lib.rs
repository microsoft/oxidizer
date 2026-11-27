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
//! An asynchronous worker can provide snapshots of tasks in its native task
//! storage. A blocking worker reports lifecycle events only. Blocking tasks do
//! not appear in task snapshots.
//!
//! Both task types use the shared `Task*` event kinds. The enqueue methods
//! return lightweight tokens when runtime task recording is enabled. The caller
//! moves each token through queue, execution, poll, and outcome transitions.
//! Dropping an unfinished token records cancellation.
//!
//! [`TaskIdentity`] lets the runtime assign an identity before it records the
//! task. Event recording retains only events emitted after recording starts.
//! It does not reconstruct earlier task history.
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
//! let queued = runtime.enqueue_async(None, identity, None, 0, 0).unwrap();
//! let task = queued.activated(&worker);
//! let poll = task.poll_started(&worker);
//! assert!(
//!     poll.finished(&worker, Some(TaskOutcome::Completed))
//!         .is_none()
//! );
//! ```

pub mod asynchronous;
pub mod blocking;
pub mod snapshot;

mod collection;
mod runtime;
mod task;
mod task_identity;
mod task_outcome;
mod worker;

pub use runtime::{Runtime, RuntimeBuilder};
pub use task_identity::TaskIdentity;
pub use task_outcome::TaskOutcome;
pub use worker::{WorkerMetadata, WorkerOrigin, WorkerRole};
