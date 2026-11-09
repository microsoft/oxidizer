// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creating, configuring, and stopping a runtime.
//!
//! Use [`Runtime`] to run async tasks from synchronous code and control
//! when workers stop. [`Runtime::new`] starts the default configuration.
//! Each async task stays on one worker thread until it finishes or is cancelled.
//! Arty can pass values between workers through
//! [`ThreadAware`](crate::core::ThreadAware), which lets values update their state
//! for the receiving worker.
//!
//! A blocking pool is a set of threads for blocking functions.
//! [`Runtime::builder`] lets you choose workers, blocking pools, and clocks
//! before they start.
//!
//! Each task receives [`Context`](crate::task::Context) with its worker's
//! scheduler and clock. [`Runtime::scheduler`] lets the runtime place new
//! tasks, while [`Context::scheduler`](crate::task::Context::scheduler)
//! keeps children on their parent's worker.
//! [`Context::control`](crate::task::Context::control) lists the workers and
//! can request shutdown without calling `Runtime::stop`.
//!
//! Keep the `Runtime` value alive until required tasks finish.
//! See [Shutdown](#shutdown) for what happens when the runtime stops.
//!
//! # Configuration
//!
//! [`RuntimeBuilder`] collects configuration before any worker starts.
//! [`RuntimeBuilder::build`] rejects invalid configuration and starts the runtime.
//! The policy types describe each choice and its use of threads and memory.
//!
//! ## Defaults
//!
//! [`Runtime::new`] and an unchanged [`RuntimeBuilder`] use these defaults:
//!
//! - [`WorkersPolicy::default`] starts one async worker.
//! - Each async worker uses a 2 MiB stack. A larger `RUST_MIN_STACK` value
//!   overrides this default.
//! - [`BlockingPoolPolicy::default`] creates one shared blocking pool. Its
//!   default thread limits can change. Use [`BlockingPoolPolicy::max`] to set
//!   a maximum.
//! - The clock follows real time.
//! - [`MonitorPolicy::default`] starts a local Seismograph monitor when the
//!   `monitor` feature is present. Task collection runs on request. This does
//!   not enable event recording.
//! - [`PanicPolicy::default`] stops the runtime after Arty catches a panic
//!   while it creates or polls a task, or runs a blocking function.
//!
//! The [`test`](crate::test) attribute disables monitoring by default.
//! See [`main`](crate::main) and [`test`](crate::test) for their other defaults.
//!
//! ## Monitoring
//!
//! A monitor is a local connection through which the Seismograph CLI can
//! collect diagnostics. Each enabled runtime owns one connection and removes it
//! after its workers stop. [`MonitorPolicy::name`] sets the displayed runtime name.
//! Use [`MonitorPolicy::disabled`] to run without this connection or task inspection.
//!
//! Collection reads live async tasks and queued or running blocking functions
//! from their native storage. Each request limits the number of tasks per worker.
//! A blocked async worker can time out. The result reports incomplete collection
//! rather than treating missing tasks as an empty worker.
//!
//! Poll timing runs only during a requested measurement interval or event recording.
//! Completed tasks are not kept in a second task registry.
//! History that was not recorded stays unknown. A blocking function that started
//! before observation has no measured start time for that call.
//!
//! Monitor connections are separate, but the recorder and diagnostic sources are
//! process-wide. Recording, clearing event buffers, and snapshot capture through
//! one connection can affect or include other runtimes in the same process.
//! Monitor startup does not change recorder configuration or install an allocator.
//!
//! ## Policy types
//!
//! - [`WorkersPolicy`] selects how many async workers run and which processors they use.
//! - [`BlockingPoolPolicy`] selects a shared or per-worker blocking pool and
//!   sets its thread limit.
//! - [`PanicPolicy`] selects whether the runtime stops after Arty catches a panic.
//! - [`MonitorPolicy`] selects the monitoring configuration.
//!
//! [`RuntimeBuilder::stack_size`] sets the async worker stack size.
//! [`RuntimeBuilder::clock`] selects the runtime clock. See each linked API for
//! configuration requirements, resource limits, and examples.
//!
//! This example uses at most two async workers and one shared blocking pool
//! with up to four threads:
//!
//! ```
//! use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let runtime = Runtime::builder()
//!         .monitor(arty::runtime::MonitorPolicy::disabled())
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
//! the worker count, monitoring, and response to task panics. See [`main`](crate::main)
//! and [`test`](crate::test) for the supported syntax and defaults.
//!
//! Use [`RuntimeBuilder`] for custom clocks, blocking pools, stack sizes,
//! or [`WorkersPolicy::at_most`].
//! Use the builder to receive configuration errors and choose when to stop the runtime.
//!
//! # Runtime services
//!
//! The runtime provides three service groups:
//!
//! - [`task`](crate::task) explains worker selection, blocking functions, and
//!   task results.
//! - [`time`](crate::time) explains clocks, timers, and controlled test time.
//! - [`io`](crate::io) explains how to connect I/O libraries.
//!
//! # Shutdown
//!
//! [`Runtime`] owns the workers.
//! Schedulers and [`Context`](crate::task::Context) values do not keep the runtime running.
//! [`RuntimeControl::request_stop`]
//! requests shutdown without waiting. [`Runtime::stop`] takes ownership of the runtime,
//! waits for every worker, and reports shutdown failures. Dropping the `Runtime` value
//! also requests shutdown and waits, but it cannot return an error.
//!
//! Shutdown:
//!
//! - Rejects new tasks.
//! - Cancels pending async tasks.
//! - Discards queued blocking functions.
//!
//! Blocking functions that already started can finish.
//! Their completion can delay shutdown. [`JoinError`](crate::task::JoinError)
//! reports task cancellation. If a runtime failure started shutdown, the error
//! also gives access to that failure.
//!
//! Keep the `Runtime` value on a thread that is not an Arty worker.
//! Stopping or dropping it in an Arty task or on a blocking worker panics.
//! Worker code must use
//! [`RuntimeControl::request_stop`].

mod bootstrap;
mod builder;
pub(crate) mod config;
mod control;
pub(crate) mod dispatch;
mod error;
mod handle;
#[cfg(feature = "monitor")]
pub(crate) mod monitoring;
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
