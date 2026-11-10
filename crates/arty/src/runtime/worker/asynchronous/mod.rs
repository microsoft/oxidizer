// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Asynchronous worker execution and thread lifetime.
//!
//! `thread` starts the worker and coordinates activation and readiness.
//! `worker` runs tasks, timers, I/O drivers, commands, and shutdown.
//! `local_worker` stores thread-local worker state. `command` and `signal`
//! define communication with the worker.

mod command;
mod local_worker;
mod signal;
mod thread;
mod worker;

pub(crate) use command::{CommandReceiver, CommandSender, QueuedFactory, QueuedIoAccess, SpawnCommand, SystemCommand, command_channel};
pub(crate) use local_worker::LocalWorker;
pub(crate) use signal::Signal;
pub(crate) use thread::{StartOptions, start};
pub(crate) use worker::Worker;
