// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime construction, configuration, and task capabilities.
//!
//! Use [`Runtime`] to run asynchronous work from synchronous code and control
//! when workers stop. [`Runtime::new`] starts the default configuration;
//! [`Runtime::builder`] lets you choose processors, blocking pools, clocks, and
//! a telemetry sink before starting workers.
//!
//! Each task receives [`Builtins`](crate::task::Builtins) containing its worker's
//! scheduler and clock. [`Runtime::scheduler`] distributes submissions
//! across workers, while [`Builtins::scheduler`](crate::task::Builtins::scheduler)
//! keeps children on their parent's worker. [`RuntimeOperations`] can request
//! shutdown or pin an external thread to a worker's processors.
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
//! async fn main(cx: arty::task::Builtins) -> Result<(), arty::task::JoinError> {
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
mod operations;
pub(crate) mod telemetry;
pub(crate) mod thread;
mod worker;

#[doc(inline)]
pub use builder::RuntimeBuilder;
#[doc(inline)]
pub use config::{BlockingPoolPolicy, ProcessorCount};
#[doc(inline)]
pub use error::Error;
#[doc(inline)]
pub use handle::Runtime;
#[doc(inline)]
pub use operations::RuntimeOperations;

/// Implementation details for the runtime entry-point macros.
#[cfg(feature = "macros")]
#[doc(hidden)]
pub mod __private {
    #[cfg(any(test, feature = "test-util"))]
    pub use crate::time::ClockControl;

    /// Preserves an entry point's return type while reporting root-task failure.
    pub fn resume_error(error: super::Error) -> ! {
        match error.into_source().downcast::<crate::task::JoinError>() {
            Ok(error) => error.resume(),
            Err(error) => std::panic::resume_unwind(Box::new(error.to_string())),
        }
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    mod tests {
        #[test]
        fn non_join_failure_preserves_its_diagnostic_payload() {
            let payload = std::panic::catch_unwind(|| {
                super::resume_error(crate::runtime::Error::new("runtime control failed"));
            })
            .unwrap_err();
            assert_eq!(*payload.downcast::<String>().unwrap(), "runtime control failed");
        }
    }
}
