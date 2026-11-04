// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Output injection middleware for services, applications, and libraries.
//!
//! This module replaces service output at a configured rate.
//! It provides these main types:
//!
//! - [`Injection`] wraps an inner service and injects output.
//! - [`InjectionLayer`] configures and creates the injection middleware.
//!
//! # Quick start
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::injection::Injection;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock).name("my_service");
//!
//! let stack = (
//!     Injection::layer("injection", &context)
//!         .rate(0.1) // Inject output for 10% of requests.
//!         .output_with(|_input, _args| "injected error".to_string()),
//!     Execute::new(my_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn my_operation(input: String) -> String { input }
//! ```
//!
//! # Configuration
//!
//! [`InjectionLayer`] uses the type state pattern.
//! A type state records builder state in the type system.
//! The layer cannot build until you set these properties:
//!
//! - [`rate`][InjectionLayer::rate] or [`rate_with`][InjectionLayer::rate_with]:
//!   sets the injection rate in `[0.0, 1.0]`.
//! - [`output_with`][InjectionLayer::output_with], [`output`][InjectionLayer::output],
//!   [`output_error_with`][InjectionLayer::output_error_with], or
//!   [`output_error`][InjectionLayer::output_error]:
//!   creates the injected output.
//!
//! Each injection layer requires an identifier for telemetry.
//! Use `snake_case` for this identifier.
//!
//! # Output factory
//!
//! The injected output can be produced in two ways:
//!
//! - [`output_with`][InjectionLayer::output_with] receives consumed input and [`InjectionOutputArgs`].
//! - [`output`][InjectionLayer::output] clones a fixed value.
//! - [`output_error_with`][InjectionLayer::output_error_with] wraps the returned error in `Err`.
//! - [`output_error`][InjectionLayer::output_error] clones an error and wraps it in `Err`.
//!
//! # Defaults
//!
//! | Parameter | Default value | Description | Set by |
//! |-----------|---------------|-------------|---------------|
//! | Rate | `None` (required) | Probability of injection | [`rate`][InjectionLayer::rate], [`rate_with`][InjectionLayer::rate_with] |
//! | Output | `None` (required) | Produces the injected output | [`output_with`][InjectionLayer::output_with], [`output`][InjectionLayer::output], [`output_error_with`][InjectionLayer::output_error_with], [`output_error`][InjectionLayer::output_error] |
//! | Enable condition | Always enabled | Injection is applied to all requests | [`enable_if`][InjectionLayer::enable_if], [`enable_always`][InjectionLayer::enable_always], [`disable`][InjectionLayer::disable] |
//!
//! # Thread safety
//!
//! [`Injection`] implements `Send` and `Sync` as required by its `Service` implementation.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - Metric: `resilience.event` (counter)
//! - Emission: When the middleware injects output
//! - Attributes:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Injection identifier from [`Injection::layer`]
//!   - `resilience.event.name`: `chaos_injection`
//!
//! # Examples
//!
//! ## Basic use
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::injection::Injection;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Injection::layer("my_injection", &context)
//!         .rate(0.05) // Inject output for 5% of requests.
//!         .output("injected_value".to_string()),
//!     Execute::new(execute_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_operation(input: String) -> String { input }
//! ```
//!
//! ## Configuration
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::injection::{Injection, InjectionConfig};
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//! let mut config = InjectionConfig::default();
//! config.rate = 0.1;
//!
//! let stack = (
//!     Injection::layer("my_injection", &context)
//!         .config(&config)
//!         .output_with(|_input, _args| "injected".to_string()),
//!     Execute::new(execute_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_operation(input: String) -> String { input }
//! ```
//!
//! ## Conditional injection
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::injection::Injection;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Injection::layer("my_injection", &context)
//!         .rate(0.2)
//!         .output_with(|_input, _args| "injected".to_string())
//!         .enable_if(|input: &String| !input.starts_with("bypass_")),
//!     Execute::new(execute_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_operation(input: String) -> String { input }
//! ```

mod args;
mod callbacks;
mod config;
mod layer;
mod service;

#[cfg(any(feature = "metrics", test))]
mod telemetry;

pub use args::{InjectionOutputArgs, InjectionRateArgs};
pub(crate) use callbacks::{InjectionOutput, InjectionRate};
pub use config::InjectionConfig;
pub use layer::InjectionLayer;
pub use service::Injection;
#[cfg(feature = "tower-service")]
pub use service::InjectionFuture;
pub(crate) use service::InjectionShared;
