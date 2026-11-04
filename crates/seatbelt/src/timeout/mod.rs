// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Timeout middleware for services, applications, and libraries.
//!
//! This module cancels operations that take too long.
//! It provides these main types:
//!
//! - [`Timeout`] wraps an inner service and applies the timeout.
//! - [`TimeoutLayer`] configures and creates the timeout middleware.
//!
//! # Quick start
//!
//! ```rust
//! # use std::time::Duration;
//! # use std::io;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::timeout::Timeout;
//! # use seatbelt::ResilienceContext;
//! # async fn example(clock: Clock) -> Result<(), io::Error> {
//! let context = ResilienceContext::new(&clock).name("my_service");
//!
//! let stack = (
//!     Timeout::layer("timeout", &context)
//!         .timeout_error(|_| io::Error::new(io::ErrorKind::TimedOut, "operation timed out"))
//!         .timeout(Duration::from_secs(30)),
//!     Execute::new(my_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # Ok(())
//! # }
//! # async fn my_operation(input: String) -> Result<String, io::Error> { Ok(input) }
//! ```
//!
//! # Configuration
//!
//! [`TimeoutLayer`] uses the type state pattern.
//! A type state records builder state in the type system.
//! The layer cannot build until you set these required properties:
//!
//! - [`timeout_output`][TimeoutLayer::timeout_output] or [`timeout_error`][TimeoutLayer::timeout_error] creates output after a timeout.
//! - [`timeout`][TimeoutLayer::timeout] sets the time limit.
//!
//! Each timeout layer requires an identifier for telemetry.
//! Use `snake_case` for this identifier.
//!
//! Set the default timeout with [`TimeoutLayer::timeout`].
//! Override it for one request with [`TimeoutLayer::timeout_override`].
//!
//! # Defaults
//!
//! The timeout middleware uses these values when you do not provide optional configuration:
//!
//! | Parameter | Default value | Description | Set by |
//! |-----------|---------------|-------------|---------------|
//! | Timeout duration | `None` (required) | Longest time to wait for an operation | [`timeout`][TimeoutLayer::timeout] |
//! | Timeout output | `None` (required) | Output to return after a timeout | [`timeout_output`][TimeoutLayer::timeout_output], [`timeout_error`][TimeoutLayer::timeout_error] |
//! | Timeout override | `None` | Uses the default timeout for all requests | [`timeout_override`][TimeoutLayer::timeout_override] |
//! | Timeout callback | `None` | Does not run a callback | [`on_timeout`][TimeoutLayer::on_timeout] |
//! | Enable condition | Always enabled | Applies a timeout to all requests | [`enable_if`][TimeoutLayer::enable_if], [`enable_always`][TimeoutLayer::enable_always], [`disable`][TimeoutLayer::disable] |
//!
//! You must set the timeout duration and output function.
//! These values do not have defaults that work for all services.
//!
//! # Thread safety
//!
//! [`Timeout`] implements `Send` and `Sync` as required by its `Service` implementation.
//! You can share the middleware across threads.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - Metric: `resilience.event` (counter)
//! - Emission: When the inner service does not finish before the timeout
//! - Attributes:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Timeout identifier from [`Timeout::layer`]
//!   - `resilience.event.name`: `timeout`
//!
//! # Examples
//!
//! ## Basic use
//!
//! This example configures and uses timeout middleware.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::timeout::Timeout;
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! // Create shared middleware configuration with a clock for the runtime.
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Timeout::layer("my_timeout", &context)
//!         // Create the output to return after a timeout.
//!         .timeout_output(|args| {
//!             format!("timeout error, duration: {}ms", args.timeout().as_millis())
//!         })
//!         // Set the timeout duration.
//!         .timeout(Duration::from_secs(30)),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build the service.
//! let service = stack.into_service();
//!
//! // Execute the service.
//! let result = service.execute("quick".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//!
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```
//!
//! ## Advanced use
//!
//! This example uses `Result` output, custom configuration, and timeout overrides.
//!
//! ```rust
//! # use std::time::Duration;
//! # use std::io;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::timeout::Timeout;
//! # async fn example(clock: Clock) -> Result<(), io::Error> {
//! // Create shared middleware configuration.
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Timeout::layer("my_timeout", &context)
//!         // Return an error after a timeout.
//!         .timeout_error(|args| io::Error::new(io::ErrorKind::TimedOut, "request timed out"))
//!         // Set the default timeout.
//!         .timeout(Duration::from_secs(30))
//!         // Run a callback after a timeout.
//!         .on_timeout(|_output: &Result<String, io::Error>, args| {
//!             println!("timeout occurred after {}ms", args.timeout().as_millis());
//!         })
//!         // Override the timeout for selected input. Use the default for `None`.
//!         .timeout_override(|input: &String, args| {
//!             match input.as_str() {
//!                 "quick" => Some(Duration::from_secs(5)), // Use a shorter timeout.
//!                 "slow" => Some(Duration::from_secs(60)),
//!                 _ => None, // Use the default from `args.default_timeout()`.
//!             }
//!         })
//!         // Disable timeouts for selected input.
//!         .enable_if(|input: &String| !input.starts_with("bypass_")),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build and execute the service.
//! let service = stack.into_service();
//! let result = service.execute("quick".to_string()).await?;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, io::Error> { Ok(input) }
//! ```
mod args;
mod callbacks;
mod config;
mod layer;
mod service;

#[cfg(any(feature = "metrics", test))]
mod telemetry;

pub use args::{OnTimeoutArgs, TimeoutOutputArgs, TimeoutOverrideArgs};
pub(crate) use callbacks::{OnTimeout, TimeoutOutput, TimeoutOverride};
pub use config::TimeoutConfig;
pub use layer::TimeoutLayer;
pub use service::Timeout;
#[cfg(feature = "tower-service")]
pub use service::TimeoutFuture;
pub(crate) use service::TimeoutShared;
