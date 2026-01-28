// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retry resilience middleware for services, applications, and libraries.
//!
//! This module provides automatic retry capabilities with configurable backoff strategies,
//! jitter, recovery classification, and comprehensive telemetry. The primary types are
//! [`Retry`] and [`RetryLayer`]:
//!
//! - [`Retry`] is the middleware that wraps an inner service and automatically retries failed operations
//! - [`RetryLayer`] is used to configure and construct the retry middleware
//!
//! # Quick Start
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
//! The [`RetryLayer`] uses a type state pattern to enforce that all required properties are
//! configured before the layer can be built. This compile-time safety ensures that you cannot
//! accidentally create a retry layer without properly specifying input cloning and recovery logic:
//!
//! - [`clone_input_with`][RetryLayer::clone_input_with]: Required function to clone inputs for retries (Rust ownership requirement)
//! - [`recovery`][RetryLayer::recovery]: Required function to determine if an output should trigger a retry
//!
//! Each retry layer requires an identifier for telemetry purposes. This identifier should use
//! `snake_case` naming convention to maintain consistency across the codebase.
//!
//! # Thread Safety
//!
//! The [`Retry`] type is thread-safe and implements both `Send` and `Sync` as enforced by
//! the `Service` trait it implements. This allows retry middleware to be safely shared
//! across multiple threads and used in concurrent environments.
//!
//! # Retry Delay
//!
//! Retry delays are determined by the following priority order:
//!
//! 1. **Recovery Delay Override**: If [`RetryLayer::recovery_with`] returns
//!    [`RecoveryInfo::retry().delay()`][crate::RecoveryInfo::delay] or [`RecoveryInfo::unavailable()`][crate::RecoveryInfo::unavailable] with a specific duration, this delay
//!    is used directly.
//!
//! 2. **Backoff Strategy**: When no recovery delay is specified, delays are calculated using
//!    the configured backoff strategy (Constant, Linear, or Exponential with default `10ms` base delay).
//!
//! # Defaults
//!
//! The retry middleware uses the following default values when optional configuration is not provided:
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Max retry attempts | `3` (4 total) | Maximum retry attempts plus original call | [`max_retry_attempts`][RetryLayer::max_retry_attempts] |
//! | Base delay | `10` milliseconds | Base delay used for backoff calculations | [`base_delay`][RetryLayer::base_delay] |
//! | Backoff strategy | `Exponential` | Exponential backoff with base multiplier of 2 | [`backoff`][RetryLayer::backoff] |
//! | Jitter | `Enabled` | Adds randomness to delays to prevent thundering herds | [`use_jitter`][RetryLayer::use_jitter] |
//! | Max delay | `None` | No limit on maximum delay between retries | [`max_delay`][RetryLayer::max_delay] |
//! | Enable condition | Always enabled | Retry protection is applied to all requests | [`enable_if`][RetryLayer::enable_if], [`enable_always`][RetryLayer::enable_always], [`disable`][RetryLayer::disable] |
//!
//! The default base delay is optimized for **service-to-service** communication where low latency
//! is critical. For **client-to-service** scenarios (e.g., mobile apps, web frontend), consider
//! increasing the base delay to 1-2 seconds using [`base_delay`][RetryLayer::base_delay].
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted for each attempt that should be retried (including the final retry attempt)
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Timeout identifier from [`Retry::layer`]
//!   - `resilience.event.name`: Always `retry`
//!   - `resilience.attempt.index`: Attempt index (0-based)
//!   - `resilience.attempt.is_last`: Boolean indicating if this is the last retry attempt
//!
//! # Examples
//!
//! ## Basic Usage
//!
//! This example demonstrates the basic usage of configuring and using retry middleware.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::retry::{Retry, Backoff};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! // Define common options for resilience middleware. The clock is runtime-specific and
//! // must be provided. See its documentation for details.
//! let context = ResilienceContext::new(&clock).name("example");
//!
//! let stack = (
//!     Retry::layer("my_retry", &context)
//!         // Required: how to clone inputs for retries
//!         .clone_input()
//!         // Required: determine if we should retry based on output
//!         .recovery_with(|output: &Result<String, String>, _args| match output {
//!             // These are demonstrative, real code will have more meaningful recovery detection
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(msg) if msg.contains("transient") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(),
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build the service
//! let service = stack.into_service();
//!
//! // Execute the service
//! let result = service.execute("test input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```
//!
//! ## Advanced Usage
//!
//! This example demonstrates advanced usage of the retry middleware, including custom backoff
//! strategies, delay generators, and retry callbacks.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use std::io;
//! # use layered::{Execute, Stack, Service};
//! # use seatbelt::retry::{Retry, Backoff};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! // Define common options for resilience middleware.
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
//!         // Optional configuration
//!         .max_retry_attempts(5)
//!         .base_delay(Duration::from_millis(200))
//!         .backoff(Backoff::Exponential)
//!         .use_jitter(true)
//!         // Callback called just before the next retry
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
//! // Build and execute the service
//! let service = stack.into_service();
//! let result = service.execute("test_timeout".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, io::Error> { Ok(input) }
//! ```

mod args;
mod attempt;
mod backoff;
mod callbacks;
mod constants;
mod layer;
mod service;
#[cfg(any(feature = "metrics", test))]
mod telemetry;

pub use args::{CloneArgs, OnRetryArgs, RecoveryArgs, RestoreInputArgs};
pub use attempt::Attempt;
pub use backoff::Backoff;
pub(crate) use backoff::DelayBackoff;
pub(crate) use callbacks::{CloneInput, OnRetry, RestoreInput, ShouldRecover};
pub use layer::RetryLayer;
pub use service::Retry;
pub(crate) use service::RetryShared;
