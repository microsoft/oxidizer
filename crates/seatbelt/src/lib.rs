// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/seatbelt/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/seatbelt/favicon.ico")]
#![cfg_attr(
    not(all(
        feature = "retry",
        feature = "timeout",
        feature = "breaker",
        feature = "fallback",
        feature = "hedging",
        feature = "chaos-injection",
        feature = "chaos-latency",
        feature = "metrics",
        feature = "logs"
    )),
    expect(
        rustdoc::broken_intra_doc_links,
        reason = "intra-doc links break when not all features are enabled"
    )
)]

//! Resilience middleware for operations that can fail.
//!
//! # Quick start
//!
//! Use retry and timeout middleware to protect a network operation.
//! Retry repeats an operation after a temporary failure.
//! Timeout stops an operation when it takes too long.
//!
//! ```rust
//! # #[cfg(all(feature = "retry", feature = "timeout"))]
//! # {
//! # use std::time::Duration;
//! # use tick::Clock;
//! use layered::{Execute, Service, Stack};
//! use seatbelt::retry::Retry;
//! use seatbelt::timeout::Timeout;
//! use seatbelt::{RecoveryInfo, ResilienceContext};
//!
//! # async fn main(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//! let service = (
//!     // Retry temporary failures.
//!     Retry::layer("retry", &context)
//!         .clone_input()
//!         .recovery_with(|output: &String, _| match output.as_str() {
//!             "temporary_error" => RecoveryInfo::retry(),
//!             "operation timed out" => RecoveryInfo::retry(),
//!             _ => RecoveryInfo::never(),
//!         }),
//!     // Apply the time limit to each retry attempt.
//!     Timeout::layer("timeout", &context)
//!         .timeout_output(|_| "operation timed out".to_string())
//!         .timeout(Duration::from_secs(30)),
//!     // Run the operation.
//!     Execute::new(my_string_operation),
//! )
//!     .into_service();
//!
//! let result = service.execute("input data".to_string()).await;
//! # }
//! # async fn my_string_operation(input: String) -> String {
//! #     // Transform the input string.
//! #     format!("processed: {}", input)
//! # }
//! # }
//! ```
//!
//! # Purpose
//!
//! Network operations can fail for a short time.
//! A server can restart, a connection can close, or a response can take too long.
//! Seatbelt provides middleware that handles these failures in a consistent way.
//! It also provides metrics and structured logs for resilience events.
//! The middleware does not depend on a specific async runtime.
//!
//! # Overview
//!
//! This crate uses [`layered`] to combine middleware.
//! Put the layers in a tuple, then use [`Stack`][layered::Stack] to make a service.
//!
//! The middleware uses [`Clock`][tick::Clock] from [`tick`] for delays and timeouts.
//! Pass the clock to each layer through [`ResilienceContext`].
//!
//! ## Core types
//!
//! - [`ResilienceContext`] holds the shared clock and telemetry configuration.
//! - [`RecoveryInfo`] states if an output permits another attempt.
//! - [`Recovery`] lets an output type provide its own recovery classification.
//!
//! ## Middleware
//!
//! - [`timeout`] stops an operation after a time limit.
//! - [`retry`] repeats an operation after a recoverable output.
//! - [`hedging`] starts more attempts when an operation is slow.
//! - [`breaker`] blocks calls to a service that is failing.
//! - [`fallback`] replaces an invalid output with another value.
//!
//! ## Chaos testing
//!
//! The [`chaos`] module adds controlled faults to a service pipeline.
//! Use it to test how the pipeline responds to failures.
//!
//! - [`chaos::injection`] replaces service output at a configured rate.
//! - [`chaos::latency`] adds a delay before the inner service call.
//!
//! # Middleware order
//!
//! Layers run from the outside to the inside.
//! The first layer in the tuple is the outer layer.
//! Use this order for a common service pipeline:
//!
//! ```text
//! Request → [Fallback → [Retry → [Breaker → [Timeout → Operation]]]]
//! ```
//!
//! - `Fallback` supplies a value after the other layers cannot recover.
//! - `Retry` repeats the inner stack.
//! - `Breaker` blocks calls while the service is unhealthy.
//! - `Timeout` limits one attempt.
//!
//! Put `Timeout` inside `Retry`.
//! Each retry attempt then gets its own time limit.
//! If `Timeout` is outside, one time limit covers all attempts.
//!
//! # Tower compatibility
//!
//! Enable `tower-service` to use Seatbelt middleware with Tower.
//! You can then use `tower::ServiceBuilder` to combine the layers.
//!
//! ```rust
//! # fn main() {
//! # #[cfg(all(feature = "retry", feature = "timeout", feature = "tower-service"))] {
//! # use std::time::Duration;
//! # use tick::Clock;
//! use seatbelt::retry::Retry;
//! use seatbelt::timeout::Timeout;
//! use seatbelt::{RecoveryInfo, ResilienceContext};
//! use tower::ServiceBuilder;
//!
//! # async fn example(clock: Clock) {
//! let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(&clock);
//!
//! let service = ServiceBuilder::new()
//!     .layer(
//!         Retry::layer("my_retry", &context)
//!             .clone_input()
//!             .recovery_with(|result: &Result<String, String>, _| match result {
//!                 Ok(_) => RecoveryInfo::never(),
//!                 Err(_) => RecoveryInfo::retry(),
//!             }),
//!     )
//!     .layer(
//!         Timeout::layer("my_timeout", &context)
//!             .timeout(Duration::from_secs(30))
//!             .timeout_error(|_| "operation timed out".to_string()),
//!     )
//!     .service_fn(|input: String| async move { Ok::<_, String>(format!("processed: {input}")) });
//! # }
//! # }
//! # }
//! ```
//!
//! # Examples
//!
//! - [`timeout`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout.rs) limits the duration of an operation.
//! - [`timeout_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/timeout_advanced.rs) uses dynamic durations and callbacks.
//! - [`retry`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry.rs) retries recoverable output.
//! - [`retry_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_advanced.rs) clones input and adds attempt data.
//! - [`retry_outage`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/retry_outage.rs) restores input from an error.
//! - [`breaker`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/breaker.rs) opens a circuit after repeated failures.
//! - [`hedging`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/hedging.rs) starts parallel attempts for slow operations.
//! - [`fallback`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/fallback.rs) replaces invalid output.
//! - [`resilience_pipeline`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/resilience_pipeline.rs) combines retry, timeout, and metrics.
//! - [`tower`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/tower.rs) uses Tower `ServiceBuilder`.
//! - [`config`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.rs) loads a [JSON file](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/config.json).
//! - [`chaos_injection`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_injection.rs) replaces output at a configured rate.
//! - [`chaos_injection_advanced`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_injection_advanced.rs) simulates a longer outage.
//! - [`chaos_latency`](https://github.com/microsoft/oxidizer/blob/main/crates/seatbelt/examples/chaos_latency.rs) adds artificial delay.
//!
//! # Features
//!
//! Enable the features that the service needs:
//!
//! - `timeout` enables [`timeout`].
//! - `retry` enables [`retry`].
//! - `hedging` enables [`hedging`].
//! - `breaker` enables [`breaker`].
//! - `fallback` enables [`fallback`].
//! - `chaos-injection` enables [`chaos::injection`].
//! - `chaos-latency` enables [`chaos::latency`].
//! - `metrics` enables the OpenTelemetry metrics API.
//! - `logs` enables structured logs through `tracing`.
//! - `serde` adds serialization for configuration types.
//! - `tower-service` adds [`tower_service::Service`] implementations.

#[doc(inline)]
pub use recoverable::{Recovery, RecoveryInfo, RecoveryKind};

mod context;
pub use context::ResilienceContext;

pub(crate) mod attempt;
pub use attempt::Attempt;

pub mod typestates;

#[cfg(any(feature = "timeout", test))]
pub mod timeout;

#[cfg(any(feature = "retry", test))]
pub mod retry;

#[cfg(any(feature = "breaker", test))]
pub mod breaker;

#[cfg(any(feature = "fallback", test))]
pub mod fallback;

#[cfg(any(feature = "hedging", test))]
pub mod hedging;

#[cfg(any(feature = "chaos-injection", feature = "chaos-latency", test))]
pub mod chaos;

#[cfg(any(
    feature = "retry",
    feature = "breaker",
    feature = "chaos-injection",
    feature = "chaos-latency",
    test
))]
mod rnd;

#[cfg(any(
    feature = "retry",
    feature = "breaker",
    feature = "timeout",
    feature = "fallback",
    feature = "hedging",
    feature = "chaos-injection",
    feature = "chaos-latency",
    test
))]
pub(crate) mod utils;

#[cfg(any(feature = "metrics", test))]
mod metrics;

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
pub(crate) mod testing;

pub(crate) type TelemetryString = std::borrow::Cow<'static, str>;

// Installs a silent, always-interested global `tracing` subscriber before any
// unit test in this crate runs. This keeps `tracing` emission paths executing
// deterministically (never poisoned into the "disabled" state) and lets per-test
// thread-local subscribers compose safely. See `docs/tracing-tests.md`.
#[cfg(test)]
testing_aids::init_tracing!();
