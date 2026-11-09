// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime-owned Seismograph connections and native task inspection.

mod asynchronous;
mod runtime;
mod task;

#[cfg(all(test, not(miri)))]
mod tests;

pub(crate) use asynchronous::WorkerDiagnostics;
pub(crate) use runtime::RuntimeDiagnostics;
pub(crate) use task::{TaskInfo, add_task, add_task_with_metadata, current_task, enter_current};
