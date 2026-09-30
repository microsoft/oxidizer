// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction, worker ownership, and thread-aware capabilities.
//!
//! A [`Runtime`] owns worker startup and shutdown. Its [`task_scheduler`](Runtime::task_scheduler)
//! returns a detached [`TaskScheduler`](crate::task::TaskScheduler), while a task's
//! [`Builtins::scheduler`] preserves worker affinity. Task submission and join handles
//! live in [`crate::task`].
//!
//! Use [`Runtime::new`] for the default configuration or [`Runtime::builder`] to
//! select processors, blocking pools, a clock, and a telemetry sink. Configuration
//! types are available directly in this module.
//!
//! Shutdown cancels pending work and waits for already-running blocking calls.
//! Cancelled joins return [`JoinError`](crate::task::JoinError). Retaining a
//! scheduler does not keep the runtime running. Read [`Runtime`]'s destruction
//! rules before transferring the owner to another thread.
//!
//! Configuration, lifecycle, telemetry, and thread-awareness guides are available
//! through the crate's [documentation section](crate#documentation).
//! For the capabilities passed to tasks, start with [`Builtins`].

pub(crate) mod blocking_worker;
mod bootstrap;
mod builder;
pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod dispatch;
mod error;
mod handle;
pub(crate) mod telemetry;
pub(crate) mod thread;
mod worker;

pub use builder::RuntimeBuilder;
pub use config::{BlockingPoolPolicy, ProcessorCount};
pub use context::Builtins;
pub use context::operations::RuntimeOperations;
pub use error::Error;
pub use handle::Runtime;

/// Implementation details for the runtime entry-point macros.
#[cfg(feature = "macros")]
#[doc(hidden)]
pub mod __private {
    #[cfg(any(test, feature = "test-util"))]
    pub use crate::time::ClockControl;

    /// Preserves an entry point's return type while reporting root-task failure.
    pub fn resume_join_error(error: crate::task::JoinError) -> ! {
        error.resume()
    }
}
