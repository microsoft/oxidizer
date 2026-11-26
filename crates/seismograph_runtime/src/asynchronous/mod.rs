// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Asynchronous worker and task lifecycle instrumentation.
//!
//! A runtime adds a [`QueuedTask`] to a queue. Dropping that token before activation
//! records cancellation. Activation creates a [`Task`]. Each poll consumes the
//! task and returns it only while the task remains pending.

mod task;
mod worker;

pub use task::{Poll, QueuedTask, Task};
pub use worker::Worker;

pub use crate::task::TaskRequest;
