// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Asynchronous worker execution and thread lifetime.
//!
//! `thread` starts the worker and coordinates activation and readiness.
//! `worker` runs tasks, timers, I/O drivers, commands, and shutdown.
//! `local_worker` stores thread-local worker state. `protocol` and `signal`
//! define communication with the worker.

mod local_worker;
mod protocol;
mod signal;
mod thread;
mod worker;

pub(crate) use local_worker::LocalWorker;
pub(crate) use protocol::{Command, QueuedFactory, QueuedIoAccess};
pub(crate) use signal::Signal;
pub(crate) use thread::{StartOptions, start};
pub(crate) use worker::Worker;
