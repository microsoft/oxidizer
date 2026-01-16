// Copyright (c) Microsoft Corporation.

//! Timeout resilience middleware for services, applications, and libraries.
//!
//! This module provides timeout functionality to cancel long-running operations and prevent
//! services from hanging indefinitely when processing requests. The primary types are
//! [`Timeout`] and [`TimeoutLayer`]:
//!
//! - [`Timeout`] is the middleware that wraps an inner service and enforces timeout behavior
//! - [`TimeoutLayer`] is used to configure and construct the timeout middleware
//!
//! # Quick Start
//!
//! ```rust
//! # use std::time::Duration;
//! # use std::io;
//! # use oxidizer_rt::Builtins;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::timeout::Timeout;
//! # use seatbelt::SeatbeltOptions;
//! # #[oxidizer_rt::test]
//! # async fn example(state: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! # let clock = state.clock().clone();
//! let options = SeatbeltOptions::new(&clock).pipeline_name("my_service");
//!
//! let stack = (
//!     Timeout::layer("timeout", &options)
//!         .timeout_error(|_| io::Error::new(io::ErrorKind::TimedOut, "operation timed out"))
//!         .timeout(Duration::from_secs(30)),
//!     Execute::new(my_operation),
//! );
//!
//! let service = stack.build();
//! let result = service.execute("input".to_string()).await;
//! # Ok(())
//! # }
//! # async fn my_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```
//!
//! # Configuration
//!
//! The [`TimeoutLayer`] uses a type state pattern to enforce that all required properties are
//! configured before the layer can be built. This compile-time safety ensures that you cannot
//! accidentally create a timeout layer without properly specifying the timeout duration
//! and the timeout output generator:
//!
//! - [`timeout_output`][TimeoutLayer::timeout_output] or [`timeout_error`][TimeoutLayer::timeout_error]: Required function to generate output when timeout occurs
//! - [`timeout`][TimeoutLayer::timeout]: Required timeout duration for operations
//!
//! Each timeout layer requires an identifier for telemetry purposes. This identifier should use
//! `snake_case` naming convention to maintain consistency across the codebase.
//!
//! The default timeout is configured via [`TimeoutLayer::timeout`]. You can override that
//! per request with [`TimeoutLayer::timeout_override`].
//!
//! # Defaults
//!
//! The timeout middleware uses the following default values when optional configuration is not provided:
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Timeout duration | `None` (required) | Maximum duration to wait for operation completion | [`timeout`][TimeoutLayer::timeout] |
//! | Timeout output | `None` (required) | Output value to return when timeout occurs | [`timeout_output`][TimeoutLayer::timeout_output], [`timeout_error`][TimeoutLayer::timeout_error] |
//! | Timeout override | `None` | Uses default timeout for all requests | [`timeout_override`][TimeoutLayer::timeout_override] |
//! | On timeout callback | `None` | No observability by default | [`on_timeout`][TimeoutLayer::on_timeout] |
//! | Enable condition | Always enabled | Timeout protection is applied to all requests | [`enable_if`][TimeoutLayer::enable_if], [`enable_always`][TimeoutLayer::enable_always], [`disable`][TimeoutLayer::disable] |
//!
//! Unlike other middleware, timeout requires explicit configuration of both the timeout duration
//! and the output generator function, as there are no reasonable universal defaults for these values.
//!
//! # Thread Safety
//!
//! The [`Timeout`] type is thread-safe and implements both `Send` and `Sync` as enforced by
//! the `Service` trait it implements. This allows timeout middleware to be safely shared
//! across multiple threads and used in concurrent environments.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted when a timeout occurs
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`SeatbeltOptions::pipeline_name`]
//!   - `resilience.strategy.name`: Timeout identifier from [`Timeout::layer`]
//!   - `resilience.event.name`: Always `timeout`
//!
//! # Examples
//!
//! ## Basic Usage
//!
//! This example demonstrates the basic usage of configuring and using timeout middleware.
//!
//! ```rust
//! # use std::time::Duration;
//! # use oxidizer_rt::Builtins;
//! # use layered::{Execute, Service, Stack};
//! # use tick::Clock;
//! # use seatbelt::SeatbeltOptions;
//! # use seatbelt::timeout::Timeout;
//! # #[oxidizer_rt::test]
//! # async fn example(state: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! # let clock = state.clock().clone();
//! // Define common options for resilience middleware. The clock is runtime-specific and
//! // must be provided. See its documentation for details.
//! let options = SeatbeltOptions::new(&clock);
//!
//! let stack = (
//!     Timeout::layer("my_timeout", &options)
//!         // Required: timeout middleware needs to know what output to return when timeout occurs
//!         .timeout_output(|args| {
//!             format!("timeout error, duration: {}ms", args.timeout().as_millis())
//!         })
//!         // Required: timeout duration must be set
//!         .timeout(Duration::from_secs(30)),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build the service
//! let service = stack.build();
//!
//! // Execute the service
//! let result = service.execute("quick".to_string()).await;
//! # let _result = result;
//! # }
//!
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```
//!
//! ## Advanced Usage
//!
//! This example demonstrates advanced usage of the timeout middleware, including working with
//! Result-based outputs, custom configurations, and timeout overrides.
//!
//! ```rust
//! # use std::time::Duration;
//! # use std::io;
//! # use oxidizer_rt::Builtins;
//! # use layered::{Execute, Service, Stack};
//! # use tick::Clock;
//! # use seatbelt::SeatbeltOptions;
//! # use seatbelt::timeout::Timeout;
//! # #[oxidizer_rt::test]
//! # async fn example(state: Builtins) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! # let clock = state.clock().clone();
//! // Define common options for resilience middleware.
//! let options = SeatbeltOptions::new(&clock);
//!
//! let stack = (
//!     Timeout::layer("my_timeout", &options)
//!         // Return an error for Result outputs on timeout
//!         .timeout_error(|args| io::Error::new(io::ErrorKind::TimedOut, "request timed out"))
//!         // Default timeout
//!         .timeout(Duration::from_secs(30))
//!         // Callback for when a timeout occurs
//!         .on_timeout(|_output, args| {
//!             println!("timeout occurred after {}ms", args.timeout().as_millis());
//!         })
//!         // Provide per-input timeout overrides (fallback to default on None)
//!         .timeout_override(|input, args| {
//!             match input.as_str() {
//!                 "quick" => Some(Duration::from_secs(5)), // override
//!                 "slow" => Some(Duration::from_secs(60)),
//!                 _ => None, // use default (args.default_timeout())!
//!             }
//!         })
//!         // Optionally disable timeouts for some inputs
//!         .enable_if(|input| !input.starts_with("bypass_")),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build and execute the service
//! let service = stack.build();
//! let result = service.execute("quick".to_string()).await?;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```
//!
//! ## Incomplete Configuration
//!
//! This example demonstrates what happens when the `TimeoutLayer` is not fully configured.
//! The code below will not compile because the timeout layer is missing required configuration.
//!
//! ```compile_fail
//! # use std::time::Duration;
//! # use layered::{Execute, Stack};
//! # use tick::Clock;
//! # use seatbelt::SeatbeltOptions;
//! # use seatbelt::timeout::Timeout;
//! # fn example(service_options: SeatbeltOptions<String, String>) {
//! let stack = (
//!     Timeout::layer("my_timeout", &service_options), // Missing required configuration!
//!     Execute::new(|input| async move { input })
//! );
//!
//! // This will fail to compile
//! let service = stack.build();
//! # }
//! ```
mod args;
mod callbacks;
mod layer;
mod service;
mod telemetry;

pub use args::{OnTimeoutArgs, TimeoutOutputArgs, TimeoutOverrideArgs};
pub(crate) use callbacks::{OnTimeout, TimeoutOutput, TimeoutOverride};
pub use layer::TimeoutLayer;
pub use service::Timeout;
