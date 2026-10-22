// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retry middleware for services, applications, and libraries.
//!
//! This module retries operations after recoverable output.
//! It supports backoff, jitter, recovery classification, and telemetry.
//! It provides these main types:
//!
//! - [`Retry`] wraps an inner service and retries recoverable output.
//! - [`RetryLayer`] configures and creates the retry middleware.
//!
//! # Quick start
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::retry::{Retry, Backoff};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let context = ResilienceContext::new(&clock).name("my_service");
//!
//! let stack = (
//!     Retry::layer("retry", &context)
//!         .clone_input()
//!         .recovery_with(|result, _| match result {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         }),
//!     Execute::new(my_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn my_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```
//!
//! # Configuration
//!
//! [`RetryLayer`] uses the type state pattern.
//! A type state records builder state in the type system.
//! The layer cannot build until you set these required properties:
//!
//! - [`clone_input_with`][RetryLayer::clone_input_with] provides input for each retry.
//! - [`recovery`][RetryLayer::recovery] decides whether output permits a retry.
//!
//! Each retry layer requires an identifier for telemetry.
//! Use `snake_case` for this identifier.
//!
//! # Thread safety
//!
//! [`Retry`] implements `Send` and `Sync` as required by its `Service` implementation.
//! You can share the middleware across threads.
//!
//! # Retry delay
//!
//! The middleware selects a retry delay in this order:
//!
//! 1. Recovery delay override: If [`RetryLayer::recovery_with`] returns
//!    [`RecoveryInfo::retry().delay()`][crate::RecoveryInfo::delay] or [`RecoveryInfo::unavailable()`][crate::RecoveryInfo::unavailable] with a specific duration,
//!    the middleware uses this delay.
//!
//! 2. Backoff strategy: If recovery does not set a delay, the middleware uses the configured backoff.
//!    The strategies are constant, linear, and exponential. The default base delay is `10ms`.
//!
//! # Defaults
//!
//! The retry middleware uses these values when you do not provide optional configuration:
//!
//! | Parameter | Default value | Description | Set by |
//! |-----------|---------------|-------------|---------------|
//! | Maximum retry attempts | `3` (4 total calls) | Retry attempts after the original call | [`max_retry_attempts`][RetryLayer::max_retry_attempts] |
//! | Base delay | `10` milliseconds | Base delay used for backoff calculations | [`base_delay`][RetryLayer::base_delay] |
//! | Backoff strategy | `Exponential` | Exponential backoff with base multiplier of 2 | [`backoff`][RetryLayer::backoff] |
//! | Jitter | Enabled | Adds random variation to retry delays | [`use_jitter`][RetryLayer::use_jitter] |
//! | Maximum delay | `None` | Does not limit the delay between retries | [`max_delay`][RetryLayer::max_delay] |
//! | Enable condition | Always enabled | Applies retry middleware to all requests | [`enable_if`][RetryLayer::enable_if], [`enable_always`][RetryLayer::enable_always], [`disable`][RetryLayer::disable] |
//!
//! The default base delay is for low-latency service-to-service calls.
//! For client-to-service calls, set a base delay of 1 to 2 seconds with [`base_delay`][RetryLayer::base_delay].
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - Metric: `resilience.event` (counter)
//! - Emission: Once for each recoverable output, including the final attempt
//! - Attributes:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Retry identifier from [`Retry::layer`]
//!   - `resilience.event.name`: `retry`
//!   - `resilience.attempt.index`: Attempt index (0-based)
//!   - `resilience.attempt.is_last`: Whether this is the last attempt
//!   - `resilience.attempt.recovery.kind`: The [`RecoveryKind`][crate::RecoveryKind] that triggered
//!     the retry, such as `retry` or `unavailable`
//!
//! # Examples
//!
//! ## Basic use
//!
//! This example configures and uses retry middleware.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::retry::{Retry, Backoff};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! // Create shared middleware configuration with a clock for the runtime.
//! let context = ResilienceContext::new(&clock).name("example");
//!
//! let stack = (
//!     Retry::layer("my_retry", &context)
//!         // Clone the input for each retry.
//!         .clone_input()
//!         // Use the output to decide whether to retry.
//!         .recovery_with(|output: &Result<String, String>, _args| match output {
//!             // Replace these example rules with rules for the service.
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(msg) if msg.contains("transient") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(),
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build the service.
//! let service = stack.into_service();
//!
//! // Execute the service.
//! let result = service.execute("test input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```
//!
//! ## Advanced use
//!
//! This example uses custom backoff, delay rules, and retry callbacks.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use std::io;
//! # use layered::{Execute, Stack, Service};
//! # use seatbelt::retry::{Retry, Backoff};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! // Create shared middleware configuration.
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Retry::layer("advanced_retry", &context)
//!         .clone_input()
//!         .recovery_with(|output: &Result<String, io::Error>, _args| match output {
//!             Err(err) if err.kind() == io::ErrorKind::TimedOut => RecoveryInfo::retry().delay(Duration::from_secs(60)),
//!             Err(_) => RecoveryInfo::never(),
//!             Ok(_) => RecoveryInfo::never(),
//!         })
//!         // Set optional configuration.
//!         .max_retry_attempts(5)
//!         .base_delay(Duration::from_millis(200))
//!         .backoff(Backoff::Exponential)
//!         .use_jitter(true)
//!         // Run a callback before the next retry.
//!         .on_retry(|output, args| {
//!             println!(
//!                 "retrying, attempt: {}, delay: {}ms",
//!                 args.attempt(),
//!                 args.retry_delay().as_millis(),
//!             );
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build and execute the service.
//! let service = stack.into_service();
//! let result = service.execute("test_timeout".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, io::Error> { Ok(input) }
//! ```

mod args;
mod backoff;
mod callbacks;
mod config;
mod constants;
mod layer;
mod service;
#[cfg(any(feature = "metrics", test))]
mod telemetry;

pub use args::{CloneArgs, OnRetryArgs, RecoveryArgs, RestoreInputArgs};
pub use backoff::Backoff;
pub(crate) use backoff::DelayBackoff;
pub(crate) use callbacks::{CloneInput, OnRetry, RestoreInput, ShouldRecover};
pub use config::RetryConfig;
pub use layer::RetryLayer;
pub use service::Retry;
#[cfg(feature = "tower-service")]
pub use service::RetryFuture;
pub(crate) use service::RetryShared;

#[deprecated(note = "import from `seatbelt::Attempt` instead")]
pub use crate::attempt::Attempt;
