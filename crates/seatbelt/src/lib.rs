// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/seatbelt/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/seatbelt/favicon.ico")]
#![cfg_attr(
    not(all(feature = "retry", feature = "timeout", feature = "circuit", feature = "metrics", feature = "logs")),
    expect(
        rustdoc::broken_intra_doc_links,
        reason = "too ugly to make 'live links' possible with the combination of features"
    )
)]

//! Resilience and recovery mechanisms for fallible operations.
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
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! use seatbelt::retry::Retry;
//! use seatbelt::timeout::Timeout;
//! use seatbelt::{RecoveryInfo, Context};
//!
//! # async fn main(clock: Clock) {
//! let context = Context::new(&clock);
//! let service = (
//!     // Retry middleware: Automatically retries failed operations
//!     Retry::layer("retry", &context)
//!         .clone_input()
//!         .recovery_with(|output: &String, _| match output.as_str() {
//!             "temporary_error" => RecoveryInfo::retry(),
//!             "operation timed out" => RecoveryInfo::retry(),
//!             _ => RecoveryInfo::never(),
//!         }),
//!     // Timeout middleware: Cancels operations that take too long
//!     Timeout::layer("timeout", &context)
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
//! > [`Context`] when creating middleware layers.
//!
//! > **Note**: This crate uses the [`layered`] crate for composing middleware. The middleware layers
//! > can be stacked together using tuples and built into a service using the [`Stack`] trait.
//!
//! # Why?
//!
//! This crate provides production-ready resilience middleware with excellent telemetry for building
//! robust distributed systems that can automatically handle timeouts, retries, and other failure
//! scenarios.
//!
//! - **Runtime agnostic** - Works seamlessly across any async runtime. Use the same resilience
//!   patterns across different projects and migrate between runtimes without changes.
//! - **Production-ready** - Battle-tested middleware with sensible defaults and comprehensive
//!   configuration options.
//! - **Excellent telemetry** - Built-in support for metrics and structured logging to monitor
//!   resilience behavior in production.
//!
//! # Overview
//!
//! ## Core Types
//!
//! - [`Context`] - Holds shared state for resilience middleware, including the clock.
//! - [`RecoveryInfo`] - Classifies errors as recoverable (transient) or non-recoverable (permanent).
//! - [`Recovery`] - A trait for types that can determine their recoverability.
//!
//! ## Built-in Middleware
//!
//! This crate provides built-in resilience middleware that you can use out of the box. See the documentation
//! for each module for details on how to use them.
//!
//! - [`timeout`] - Middleware that cancels long-running operations.
//! - [`retry`] - Middleware that automatically retries failed operations.
//! - [`circuit`] - Middleware that prevents cascading failures.
//!
//! # Features
//!
//! This crate provides several optional features that can be enabled in your `Cargo.toml`:
//!
//! - **`timeout`** - Enables the [`timeout`] middleware for canceling long-running operations.
//! - **`retry`** - Enables the [`retry`] middleware for automatically retrying failed operations with
//!   configurable backoff strategies, jitter, and recovery classification.
//! - **`circuit`** - Enables the [`circuit`] middleware for preventing cascading failures.
//! - **`metrics`** - Exposes the OpenTelemetry metrics API for collecting and reporting metrics.
//! - **`logs`** - Enables structured logging for resilience middleware using the `tracing` crate.

#[doc(inline)]
pub use recoverable::{Recovery, RecoveryInfo, RecoveryKind};

pub(crate) mod shared;
pub use crate::shared::{Attempt, Backoff, Context, NotSet, Set};

#[cfg(any(feature = "timeout", test))]
pub mod timeout;

#[cfg(any(feature = "retry", test))]
pub mod retry;

#[cfg(any(feature = "circuit", test))]
pub mod circuit;

#[doc(inline)]
pub use layered::{Layer, Service, Stack};

#[cfg(any(feature = "retry", feature = "circuit", test))]
mod rnd;

#[cfg(any(feature = "retry", feature = "circuit", feature = "timeout", test))]
pub(crate) mod utils;

#[cfg(any(feature = "metrics", test))]
mod metrics;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
pub(crate) mod testing;
