// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction, worker ownership, and thread-aware capabilities.
//!
//! A [`Runtime`] owns worker startup and shutdown. Its [`task_scheduler`](Runtime::task_scheduler)
//! returns a detached [`TaskScheduler`](crate::task::TaskScheduler), while a task's
//! [`Builtins::scheduler`] preserves worker affinity. Task submission and join handles
//! live in [`crate::task`].
//!
//! Dropping the runtime cancels asynchronous work and waits for accepted blocking work.
//! Cancelled joins remain pending. Retaining a scheduler does not keep the runtime
//! running. There is no asynchronous I/O or memory-pool subsystem.
//!
//! # Telemetry
//!
//! Configure [`RuntimeBuilder::sink`] to receive `observed` events. The default
//! sink is a noop. Tasks capture enrichment at submission and restore it while
//! polling. Runtime event names retain the `oxidizer.rt` prefix.
//!
//! Classified fields use the `arty/SystemMetadata` data class. Configure a
//! processor's redaction policy with `data_privacy::DataClass::new("arty", "SystemMetadata")`.
//! Numeric metric values remain unredacted numbers.
//! Opaque Rust thread identifiers are logged as `arty.thread.id`, not the
//! integer-valued OpenTelemetry `thread.id` attribute.
//! Blocking-pool telemetry retains its existing `system_worker` wire names for compatibility.

pub(crate) mod blocking_worker;
mod bootstrap;
mod builder;
pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod dispatch;
mod error;
mod handle;
#[cfg(feature = "macros")]
mod macros;
pub(crate) mod telemetry;
pub(crate) mod thread;
mod worker;

pub use builder::RuntimeBuilder;
pub use config::{ProcessorCount, WorkerPoolPolicy};
pub use context::Builtins;
pub use context::operations::RuntimeOperations;
pub use error::Error;
pub use handle::Runtime;
#[cfg(feature = "macros")]
pub use macros::{main, test};

/// Implementation details for the runtime entry-point macros.
#[cfg(feature = "macros")]
#[doc(hidden)]
pub mod __private {
    #[cfg(any(test, feature = "test-util"))]
    pub use crate::time::ClockControl;
}
