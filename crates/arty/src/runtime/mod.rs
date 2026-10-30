// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction, configuration, and shutdown.
//!
//! Use [`Runtime`] to run async work from synchronous code and control
//! when workers stop. [`Runtime::new`] starts the default configuration.
//! [`Runtime::builder`] lets you choose workers, blocking pools, and clocks
//! before they start.
//!
//! Each task receives [`Context`](crate::task::Context) with its worker's
//! scheduler and clock. [`Runtime::scheduler`] lets the runtime place new
//! tasks, while [`Context::scheduler`](crate::task::Context::scheduler)
//! keeps children on their parent's worker.
//! [`Context::control`](crate::task::Context::control) exposes worker
//! coordinates and can request shutdown without consuming the runtime owner.
//!
//! Keep the runtime owner alive until required work completes. Shutdown cancels
//! pending tasks and waits for blocking callbacks that have already started.
//! Keeping a scheduler does not keep the runtime running.
//!
//! # Configuration
//!
//! [`RuntimeBuilder`] collects configuration before any worker starts.
//! [`RuntimeBuilder::build`] validates the configuration and starts the runtime.
//! The policy types contain the detailed choices and their resource effects.
//!
//! ## Defaults
//!
//! [`Runtime::new`] and an unchanged [`RuntimeBuilder`] use these defaults:
//!
//! - [`WorkersPolicy::default`] starts one async worker.
//! - Each async worker uses a 2 MiB stack. A larger `RUST_MIN_STACK` value
//!   takes precedence.
//! - [`BlockingPoolPolicy::default`] creates one shared blocking pool. Its
//!   implicit thread limits are implementation details. Use
//!   [`BlockingPoolPolicy::max`] to set an explicit maximum.
//! - The clock follows real time.
//! - [`MonitorPolicy::default`] selects the enabled monitoring policy. This
//!   policy does not start a monitor or instrument tasks.
//! - [`PanicPolicy::default`] starts coordinated shutdown after a contained
//!   task factory, async task, or blocking callback panic.
//!
//! The [`test`](crate::test) attribute selects the disabled monitoring policy
//! by default. See [`main`](crate::main) and [`test`](crate::test) for all
//! entry-point defaults.
//!
//! ## Policy types
//!
//! - [`WorkersPolicy`] selects the async worker count and processor placement.
//! - [`BlockingPoolPolicy`] selects a shared or per-worker blocking pool and
//!   sets its thread limit.
//! - [`PanicPolicy`] selects shutdown or continued operation after a contained
//!   panic.
//! - [`MonitorPolicy`] selects the monitoring configuration.
//!
//! [`RuntimeBuilder::stack_size`] sets the async worker stack size.
//! [`RuntimeBuilder::clock`] selects the runtime clock. See each linked API for
//! validation rules, resource limits, and examples.
//!
//! This example uses at most two async workers and one shared blocking pool
//! with up to four threads:
//!
//! ```
//! use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = Runtime::builder()
//!         .workers(WorkersPolicy::at_most(2))
//!         .blocking_pool(BlockingPoolPolicy::shared().max(4))
//!         .build()?;
//!     assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
//!     runtime.stop()?;
//!     Ok(())
//! }
//! ```
//!
//! # Entry-point attributes
//!
//! With `macros` enabled, [`arty::main`](crate::main) and
//! [`arty::test`](crate::test) create and stop the runtime. Their arguments set
//! the worker count, monitoring policy, and panic policy. See [`main`](crate::main)
//! and [`test`](crate::test) for the supported syntax and defaults.
//!
//! Build the runtime explicitly for custom clocks, blocking pools, stack sizes,
//! or [`WorkersPolicy::at_most`]. Explicit construction also lets the caller
//! own shutdown and handle construction errors.
//!
//! # Runtime services
//!
//! The runtime combines three service groups. The [`task`](crate::task) module
//! documents task placement, blocking work, and result handles. The
//! [`time`](crate::time) module documents clocks, timers, and controlled test
//! time. The [`io`](crate::io) module documents external I/O driver hosting.
//!
//! # Shutdown
//!
//! [`Runtime`] owns the workers. Scheduler and [`Context`](crate::task::Context)
//! handles do not keep the runtime running. [`RuntimeControl::request_stop`]
//! requests shutdown without waiting. [`Runtime::stop`] consumes the owner,
//! waits for every worker, and reports shutdown failures. Dropping the owner
//! also requests shutdown and waits, but it cannot return an error.
//!
//! Shutdown rejects new submissions, cancels pending async tasks, and discards
//! queued blocking callbacks. Blocking callbacks that already run can finish.
//! Their completion can delay shutdown. [`JoinError`](crate::task::JoinError)
//! identifies task cancellation and preserves a runtime failure when one
//! started shutdown.
//!
//! Keep the runtime owner on an external thread. Stopping or dropping it on an
//! Arty task or blocking worker panics. Worker code must use
//! [`RuntimeControl::request_stop`].

mod bootstrap;
mod builder;
pub(crate) mod config;
mod control;
pub(crate) mod dispatch;
mod error;
mod handle;
mod shutdown;
pub(crate) mod worker;
mod workers;

#[doc(inline)]
pub use builder::RuntimeBuilder;
#[doc(inline)]
pub use config::{BlockingPoolPolicy, MonitorPolicy, PanicPolicy, WorkersPolicy};
#[doc(inline)]
pub use control::RuntimeControl;
#[doc(inline)]
pub use error::Error;
#[doc(inline)]
pub use handle::Runtime;
pub(crate) use shutdown::{Shutdown, ShutdownStart, ShutdownState};
#[doc(inline)]
pub use workers::Workers;

/// Implementation details for the runtime entry-point macros.
#[cfg(feature = "macros")]
#[doc(hidden)]
pub mod __private {
    use std::error::Error as _;

    #[cfg(any(test, feature = "test-util"))]
    pub use crate::time::ClockControl;

    /// Preserves an entry point's return type while reporting root-task failure.
    ///
    /// # Panics
    ///
    /// Resumes a captured root-task panic or starts an unwind carrying the runtime failure.
    pub fn resume_error(error: super::Error) -> ! {
        match error.into_source().downcast::<crate::task::JoinError>() {
            Ok(error) if error.is_panic() => {
                std::panic::resume_unwind(error.into_panic().expect("the join error was checked as a panic"));
            }
            Ok(error) => {
                let message = error.source().map_or_else(
                    || "runtime is shutting down".to_owned(),
                    |source| format!("runtime is shutting down: {source}"),
                );
                std::panic::resume_unwind(Box::new(message));
            }
            Err(error) => std::panic::resume_unwind(Box::new(error.to_string())),
        }
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    mod tests {
        use std::fmt;

        #[test]
        fn non_join_failure_preserves_its_diagnostic_payload() {
            let payload = std::panic::catch_unwind(|| {
                super::resume_error(crate::runtime::Error::new("runtime control failed"));
            })
            .unwrap_err();
            assert_eq!(*payload.downcast::<String>().unwrap(), "runtime control failed");
        }

        #[derive(Debug)]
        struct RuntimeFailure;

        impl fmt::Display for RuntimeFailure {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("driver execute cycle failed")
            }
        }

        impl std::error::Error for RuntimeFailure {}

        #[test]
        fn caused_shutdown_preserves_its_diagnostic_payload() {
            let join_error = crate::task::JoinError::shutdown(Some(arty_executor::ShutdownError::new(RuntimeFailure)));
            let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                super::resume_error(crate::runtime::Error::new(join_error));
            }))
            .unwrap_err();

            assert_eq!(
                *payload.downcast::<String>().unwrap(),
                "runtime is shutting down: driver execute cycle failed"
            );
        }
    }
}
