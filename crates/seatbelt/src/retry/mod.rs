// Copyright (c) Microsoft Corporation.

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
//! # use oxidizer_rt::Builtins;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::retry::Retry;
//! # use seatbelt::{Backoff, RecoveryInfo, SeatbeltOptions};
//! # #[oxidizer_rt::test]
//! # async fn example(state: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! # let clock = state.clock().clone();
//! let options = SeatbeltOptions::new(&clock).pipeline_name("my_service");
//!
//! let stack = (
//!     Retry::layer("retry", &options)
//!         .clone_input_with(|args| Some(args.input().clone()))
//!         .recovery_with(|result, _| match result {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         }),
//!     Execute::new(my_operation),
//! );
//!
//! let service = stack.build();
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
//!    the configured backoff strategy (Constant, Linear, or Exponential with default 2s base delay).
//!
//! # Defaults
//!
//! The retry middleware uses the following default values when optional configuration is not provided:
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Max retry attempts | `3` (4 total) | Maximum retry attempts plus original call | [`max_retry_attempts`][RetryLayer::max_retry_attempts], [`infinite_retry_attempts`][RetryLayer::infinite_retry_attempts] |
//! | Base delay | `2` seconds | Base delay used for backoff calculations | [`base_delay`][RetryLayer::base_delay] |
//! | Backoff strategy | `Exponential` | Exponential backoff with base multiplier of 2 | [`backoff`][RetryLayer::backoff] |
//! | Jitter | `Enabled` | Adds randomness to delays to prevent thundering herds | [`use_jitter`][RetryLayer::use_jitter] |
//! | Max delay | `None` | No limit on maximum delay between retries | [`max_delay`][RetryLayer::max_delay] |
//! | Enable condition | Always enabled | Retry protection is applied to all requests | [`enable_if`][RetryLayer::enable_if], [`enable_always`][RetryLayer::enable_always], [`disable`][RetryLayer::disable] |
//!
//! These defaults provide a reasonable starting point for most use cases, offering a balance
//! between resilience and avoiding an excessive load on downstream services.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted for each attempt that should be retried (including the final retry attempt)
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`SeatbeltOptions::pipeline_name`]
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
//! # use oxidizer_rt::Builtins;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::retry::Retry;
//! # use seatbelt::{Backoff, RecoveryInfo, SeatbeltOptions};
//! # #[oxidizer_rt::test]
//! # async fn example(state: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! # let clock = state.clock().clone();
//! // Define common options for resilience middleware. The clock is runtime-specific and
//! // must be provided. See its documentation for details.
//! let options = SeatbeltOptions::new(&clock).pipeline_name("example");
//!
//! let stack = (
//!     Retry::layer("my_retry", &options)
//!         // Required: how to clone inputs for retries
//!         .clone_input_with(|args| Some(args.input().clone()))
//!         // Required: determine if we should retry based on output
//!         .recovery_with(|output, _args| match output {
//!             // These are demonstrative, real code will have more meaningful recovery detection
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(msg) if msg.contains("transient") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(),
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build the service
//! let service = stack.build();
//!
//! // Execute the service
//! let result = service.execute("test input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok(input) }
//! ```
//!
//! ## Advanced Usage
//!
//! This example demonstrates advanced usage of the retry middleware, including custom backoff
//! strategies, delay generators, and retry callbacks.
//!
//! ```rust
//! # use std::time::Duration;
//! # use oxidizer_rt::Builtins;
//! # use layered::{Execute, Stack};
//! # use seatbelt::retry::Retry;
//! # use seatbelt::{RecoveryInfo, SeatbeltOptions, Backoff};
//! # #[oxidizer_rt::test]
//! # async fn example(state: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! # let clock = state.clock().clone();
//! // Define common options for resilience middleware.
//! let options = SeatbeltOptions::new(&clock);
//!
//! let stack = (
//!     Retry::layer("advanced_retry", &options)
//!         .clone_input_with(|args| Some(args.input().clone()))
//!         .recovery_with(|output, _args| match output {
//!             Err(msg) if msg.contains("rate_limit") => {
//!                 RecoveryInfo::retry().delay(Duration::from_secs(60))
//!             }
//!             Err(msg) if msg.contains("timeout") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(),
//!             Ok(_) => RecoveryInfo::never(),
//!         })
//!         // Optional configuration
//!         .max_retry_attempts(5)
//!         .base_delay(Duration::from_millis(200))
//!         .backoff(Backoff::Exponential)
//!         .jitter(true)
//!         // You can extract the delay from the output, or return None to use the
//!         // one provided by the retry middleware
//!         .delay_generator(|_output, args| None)
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
//! let service = stack.build();
//! let result = service.execute("test_timeout".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, Box<dyn std::error::Error + Send + Sync>> { Ok(input) }
//! ```
//!
//! ## Incomplete Configuration
//!
//! This example demonstrates what happens when the `RetryLayer` is not fully configured.
//! The code below will not compile because the retry layer is missing required configuration.
//!
//! ```compile_fail
//! # use seatbelt::retry::Retry;
//! # use seatbelt::SeatbeltOptions;
//! # use tick::Clock;
//! # fn example(service_options: SeatbeltOptions<String, String>) {
//! let stack = (
//!     Retry::layer("test", &service_options), // Missing required configuration!
//!     Execute::new(|input| async move { input })
//! );
//!
//! // This will fail to compile
//! let service = stack.build();
//! # }
//! ```
//!
//! For more comprehensive examples, see the [examples directory](https://github.com/microsoft/oxidizer/tree/main/crates/seatbelt/examples).

mod args;
mod backoff;
mod callbacks;
mod constants;
mod layer;
mod service;
mod telemetry;

pub use args::{CloneArgs, OnRetryArgs, RecoveryArgs, RestoreInputArgs};
pub(crate) use backoff::DelayBackoff;
pub(crate) use callbacks::{CloneInput, OnRetry, RestoreInput, ShouldRecover};
pub use layer::RetryLayer;
pub use service::Retry;
