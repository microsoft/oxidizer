// Copyright (c) Microsoft Corporation.

#![expect(
    rustdoc::broken_intra_doc_links,
    reason = "Too ugly to make 'live links' possible with the combination of features"
)]

//! Resilience and fault handling for applications and libraries.
//!
//! This crate helps applications handle transient faults gracefully through composable
//! resilience patterns. It provides resilience middleware for building robust distributed systems
//! that can automatically handle timeouts, retries, and other failure scenarios.
//!
//! # Runtime Agnostic Design
//!
//! The seatbelt crate is designed to be **runtime agnostic** and works seamlessly across any
//! async runtime. This flexibility allows you to use the same resilience patterns across
//! different projects and migrate between runtimes without changing your resilience patterns.
//!
//! # Core Types
//!
//! - [`RecoveryInfo`]: Classifies errors as recoverable (transient) or non-recoverable (permanent).
//! - [`Recovery`]: A trait for types that can determine their recoverability.
//!
//! # Quick Start
//!
//! Add resilience to your services with just a few lines of code. **Retry** handles transient failures
//! and **Timeout** prevents operations from hanging indefinitely:
//!
//! ```rust
//! # #[cfg(all(feature = "retry", feature = "timeout"))]
//! # {
//! # use std::time::Duration;
//! # use oxidizer_rt::Builtins;
//! # use layered::{Execute, Service, Stack};
//! use seatbelt::retry::Retry;
//! use seatbelt::timeout::Timeout;
//! use seatbelt::{RecoveryInfo, SeatbeltOptions};
//!
//! # #[oxidizer_rt::main]
//! # async fn main(state: Builtins) {
//! # let clock = state.clock().clone();
//! let options = SeatbeltOptions::new(&clock);
//! let service = (
//!     // Retry middleware: Automatically retries failed operations
//!     Retry::layer("retry", &options)
//!         .clone_input()
//!         .recovery_with(|output: &String, _| match output.as_str() {
//!             "temporary_error" => RecoveryInfo::retry(),
//!             "operation timed out" => RecoveryInfo::retry(),
//!             _ => RecoveryInfo::never(),
//!         }),
//!     // Timeout middleware: Cancels operations that take too long
//!     Timeout::layer("timeout", &options)
//!         .timeout_output(|_| "operation timed out".to_string())
//!         .timeout(Duration::from_secs(30)),
//!     // Your core business logic
//!     Execute::new(my_string_operation),
//! )
//!     .build();
//!
//! let result = service.execute("input data".to_string()).await;
//! # }
//! # async fn my_string_operation(input: String) -> String {
//! #     // Simulate processing that transforms the input string
//! #     format!("processed: {}", input)
//! # }
//! # }
//! ```
//!
//! > **Note**: Resilience middleware requires [`Clock`][tick::Clock] from the [`tick`] crate for timing
//! > operations like delays, timeouts, and backoff calculations. The clock is passed through
//! > [`SeatbeltOptions`] when creating middleware layers.
//!
//! See [Built-in Middlewares](#built-in-middleware) for more details.
//!
//! # Recovery Metadata
//!
//! Error types can implement [`Recovery`] to provide additional metadata about their retry characteristics.
//! This enables callers to use a unified, streamlined approach when determining whether to retry an
//! operation, regardless of the underlying error type or source.
//!
//! # Built-in Middleware
//!
//! This crate provides built-in resilience middleware that you can use out of the box. See the documentation
//! for each module for details on how to use them.
//!
//! - [`timeout`]: Cancels long-running operations.
//! - [`retry`]: Automatically retries failed operations with configurable backoff strategies.
//! - [`circuit_breaker`]: Prevents cascading failures by stopping requests to unhealthy services.
//!
//! ## Features
//!
//! This crate supports several optional features that can be enabled to extend functionality:
//!
//! - `options`: Enables common APIs for building resilience middleware, including [`SeatbeltOptions`].
//!   Requires [`tick`] for timing operations.
//! - `service`: Re-exports common types for building middleware from [`layered`] crate.
//! - `telemetry`: Enables telemetry and observability features using OpenTelemetry for monitoring
//!   resilience operations.
//! - `metrics`: Exposes the OpenTelemetry metrics API for collecting and reporting metrics.
//! - `timeout`: Enables the [`timeout`] middleware for canceling long-running operations.
//! - `retry`: Enables the [`retry`] middleware for automatically retrying failed operations with
//!   configurable backoff strategies, jitter, and recovery classification.
//! - `circuit-breaker`: Enables the [`circuit_breaker`] middleware for preventing cascading failures.

#[doc(inline)]
pub use recoverable::{Recovery, RecoveryInfo, RecoveryKind};

#[cfg(any(feature = "options", test))]
pub(crate) mod options;

#[cfg(any(feature = "options", test))]
pub(crate) use options::EnableIf;
#[cfg(any(feature = "options", test))]
pub(crate) use options::define_fn_wrapper;

#[cfg(any(feature = "retry", test))]
pub(crate) use crate::options::MaxAttempts;
#[cfg(any(feature = "options", test))]
pub use crate::options::{Attempt, Backoff, NotSet, SeatbeltOptions, Set};

#[cfg(any(feature = "telemetry", test))]
pub mod telemetry;

#[cfg(any(feature = "timeout", test))]
pub mod timeout;

#[cfg(any(feature = "retry", test))]
pub mod retry;

#[cfg(any(feature = "circuit-breaker", test))]
pub mod circuit_breaker;

/// Re-exported types from the [`layered`] crate.
#[cfg(any(feature = "service", test))]
pub mod service {
    #[doc(inline)]
    pub use layered::{Layer, Service, Stack};
}

#[cfg(any(feature = "retry", feature = "circuit-breaker", test))]
mod rnd;

#[cfg(test)]
pub mod testing;
