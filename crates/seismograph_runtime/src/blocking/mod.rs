// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Blocking worker and task lifecycle instrumentation.
//!
//! A runtime adds a [`QueuedTask`] to a queue. Dropping that token before execution
//! records cancellation. Starting the task creates an [`Execution`] token that
//! records queue and execution durations.

mod task;
mod worker;

pub use task::{Execution, QueuedTask, TaskHandle};
pub use worker::Worker;
