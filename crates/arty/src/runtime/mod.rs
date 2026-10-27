// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction, configuration, and task capabilities.
//!
//! Use [`Runtime`] to run async work from synchronous code and control
//! when workers stop. [`Runtime::new`] starts the default configuration;
//! [`Runtime::builder`] lets you choose workers, blocking pools, clocks, and
//! a telemetry sink before they start.
//!
//! Each task receives [`Builtins`](crate::task::Builtins) with its worker's
//! scheduler and clock. [`Runtime::scheduler`] lets the runtime place new
//! tasks, while [`Builtins::scheduler`](crate::task::Builtins::scheduler)
//! keeps children on their parent's worker.
//! [`Builtins::control`](crate::task::Builtins::control) exposes worker
//! coordinates and can request shutdown without consuming the runtime owner.
//!
//! Keep the runtime owner alive until required work completes. Shutdown cancels
//! pending tasks and waits for blocking callbacks that have already started;
//! keeping a scheduler does not keep the runtime running.
//!
//! [`PanicPolicy`] selects the intended response to a contained task panic.
//! This release stores the policy in runtime configuration but does not yet
//! change panic behavior. A follow-up change will enforce the selected policy.
//!
//! # Examples
//!
//! Let the entry-point attribute manage the runtime's lifetime:
//!
//! ```
//! # #[cfg(all(feature = "macros", feature = "rt"))]
//! #[arty::main]
//! async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
//!     assert_eq!(cx.scheduler().spawn(async |_| 42).await?, 42);
//!     Ok(())
//! }
//! # #[cfg(not(all(feature = "macros", feature = "rt")))] fn main() {}
//! ```
//!
//! See the [configuration](crate::documentation::configuration) and
//! [shutdown](crate::documentation::shutdown) guides for more.

pub(crate) mod blocking_worker;
mod bootstrap;
mod builder;
pub(crate) mod config;
mod control;
pub(crate) mod dispatch;
mod error;
mod handle;
mod shutdown;
pub(crate) mod telemetry;
pub(crate) mod thread;
mod worker;
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
pub(crate) use shutdown::{Shutdown, ShutdownState};
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
