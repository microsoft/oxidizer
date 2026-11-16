// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private task monitoring and native task inspection.

#[cfg(feature = "monitor")]
mod asynchronous;
mod blocking;
#[cfg(feature = "monitor")]
mod runtime;
mod submission;
#[cfg(feature = "monitor")]
mod task;

#[cfg(test)]
#[cfg(all(feature = "monitor", not(miri)))]
mod tests;

// Hold for the full lifetime of tests that use the process registry or recorder.
#[cfg(all(test, feature = "monitor"))]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "monitor")]
pub(crate) use asynchronous::WorkerDiagnostics;
#[cfg(feature = "monitor")]
pub(crate) use blocking::MonitoredBlockingJob;
pub(crate) use blocking::{BlockingJob, BlockingPoolMonitoring, BlockingTaskEvents, BlockingTaskMonitoring, BlockingWorkerMonitoring};
#[cfg(feature = "monitor")]
pub(crate) use runtime::RuntimeDiagnostics;
pub(crate) use submission::{TaskMonitoring, add_task};
#[cfg(feature = "monitor")]
pub(crate) use task::TaskInfo;
#[cfg(all(test, feature = "monitor"))]
pub(crate) use task::current_task;
