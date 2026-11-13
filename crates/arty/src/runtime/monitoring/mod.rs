// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-owned Seismograph connections and native task inspection.

mod asynchronous;
mod runtime;
mod task;

#[cfg(test)]
#[cfg(not(miri))]
mod tests;

// Hold for the full lifetime of tests that use the process registry or recorder.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) use asynchronous::WorkerDiagnostics;
pub(crate) use runtime::RuntimeDiagnostics;
pub(crate) use task::{TaskInfo, add_task, add_task_with_metadata, current_task, enter_current};
