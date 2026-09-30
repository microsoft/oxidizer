// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction, configuration, and task capabilities.
//!
//! Use [`Runtime`] to run asynchronous work from synchronous code and control
//! when workers stop. [`Runtime::new`] starts the default configuration;
//! [`Runtime::builder`] lets you choose processors, blocking pools, clocks, and
//! a telemetry sink before starting workers.
//!
//! Each task receives [`Builtins`] containing its worker's scheduler, clock,
//! and runtime operations. [`Runtime::task_scheduler`] distributes submissions
//! across workers, while [`Builtins::scheduler`] keeps children on their parent's
//! worker. Submission and result handles are documented in [`crate::task`].
//!
//! Keep the runtime owner alive until required work completes. Shutdown cancels
//! pending tasks and waits for blocking callbacks that have already started;
//! keeping a scheduler does not keep the runtime running.
//!
//! # Examples
//!
//! Let the entry-point attribute manage the runtime's lifetime:
//!
//! ```
//! # #[cfg(feature = "macros")]
//! #[arty::main]
//! async fn main(cx: arty::runtime::Builtins) -> Result<(), arty::task::JoinError> {
//!     assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
//!     Ok(())
//! }
//! # #[cfg(not(feature = "macros"))] fn main() {}
//! ```
//!
//! See the crate's [guides](crate#documentation) for configuration and shutdown
//! patterns, or use its entry-point attributes to manage ownership automatically.

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

#[doc(inline)]
pub use builder::RuntimeBuilder;
#[doc(inline)]
pub use config::{BlockingPoolPolicy, ProcessorCount};
#[doc(inline)]
pub use context::Builtins;
#[doc(inline)]
pub use context::operations::RuntimeOperations;
#[doc(inline)]
pub use error::Error;
#[doc(inline)]
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
