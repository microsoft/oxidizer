// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private task monitoring and native task inspection.
//!
//! The runtime owns its monitor connection and registration until its workers stop.
//! Each async worker owns a worker registration. The executor owns async task metadata.
//! Blocking queues own worker registrations and share an event context for submissions.
//! Async executor tasks own their task metadata. A blocking callback and its
//! inspection metadata share one allocation so inspection can find queued and running tasks.
//! Handles provide access to these registrations but do not own runtime shutdown.

mod asynchronous;
mod blocking;
mod blocking_task;
mod runtime;
mod task;
mod task_properties;

#[cfg(test)]
#[cfg(not(miri))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

// Hold for the full lifetime of tests that use the process registry or recorder.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) use asynchronous::WorkerMonitoring;
pub(crate) use blocking::{BlockingJob, BlockingPoolMonitoring, BlockingTaskMonitoring, BlockingWorkerMonitoring, MonitoredBlockingJob};
pub(crate) use blocking_task::BlockingTaskInfo;
pub(crate) use runtime::RuntimeMonitoring;
pub(crate) use task::{current_events, current_task};
pub(crate) use task_properties::capture_task_properties;
