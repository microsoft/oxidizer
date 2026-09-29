// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-aware task scheduling, worker ownership, and runtime capabilities.
//!
//! A [`Runtime`] owns worker startup and shutdown. [`Runtime::task_scheduler`] returns
//! a detached scheduler that distributes submissions round-robin. A task's
//! [`Builtins::scheduler`] preserves its worker affinity.
//!
//! Tasks may produce non-`Send` futures on their worker. [`LocalTaskScheduler`] also
//! accepts non-`Send` captures and results. Remote results must be `Send` and are
//! not automatically relocated.
//!
//! Dropping the runtime cancels asynchronous work and waits for shutdown and accepted
//! system work. Cancelled joins remain pending; retaining a scheduler does not keep
//! its runtime running. There is no asynchronous I/O or memory-pool subsystem.
//!
//! # Telemetry
//!
//! Configure [`config::RuntimeBuilder::sink`] to receive `observed` events. The
//! default sink is a noop. Asynchronous tasks capture enrichment at submission and
//! restore it while polling. Runtime event names retain the `oxidizer.rt` prefix.
//! Classified fields use the `arty/SystemMetadata` data class. Processors configure
//! their redaction policy with `data_privacy::DataClass::new("arty", "SystemMetadata")`.
//! Numeric metric
//! values remain unredacted numbers.

pub mod config;
mod error;
#[cfg(feature = "macros")]
mod macros;
mod runtime;
mod task;
mod telemetry;

pub use error::{Error, Result};
#[cfg(feature = "macros")]
pub use macros::{main, test};
pub use runtime::context::Builtins;
pub use runtime::context::operations::RuntimeOperations;
pub use runtime::handle::Runtime;
pub use task::join::{JoinHandle, LocalJoinHandle};
pub use task::local::LocalTaskScheduler;
pub use task::scheduler::TaskScheduler;
