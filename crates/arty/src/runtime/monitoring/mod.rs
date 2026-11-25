// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private task monitoring and native task inspection.
//!
//! The runtime owns its monitor connection and registration until its workers stop.
//! Each async worker owns a worker registration and its current poll state.
//! Blocking queues own worker registrations and share an event context for submissions.
//! Async completion targets own their task metadata. A blocking callback and its
//! inspection metadata share one allocation so inspection can find queued and running tasks.
//! Handles provide access to these registrations but do not own runtime shutdown.

mod asynchronous;
mod blocking;
mod runtime;
mod task;

#[cfg(test)]
#[cfg(not(miri))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;

// Hold for the full lifetime of tests that use the process registry or recorder.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) use asynchronous::WorkerMonitoring;
pub(crate) use blocking::{
    BlockingJob, BlockingPoolMonitoring, BlockingTaskEvents, BlockingTaskMonitoring, BlockingWorkerMonitoring, MonitoredBlockingJob,
};
pub(crate) use runtime::RuntimeMonitoring;
pub(crate) use task::{TaskInfo, current_task, descriptor, register_monitored};
#[cfg(test)]
pub(crate) use task::{enter_current, set_events};
