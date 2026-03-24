// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Chaos injection middleware for services, applications, and libraries.
//!
//! This module provides a fault injection mechanism that replaces service output
//! with a user-provided value at a configurable probability. The primary types
//! are [`Injection`] and [`InjectionLayer`]:
//!
//! - [`Injection`] is the middleware that wraps an inner service and injects output
//! - [`InjectionLayer`] is used to configure and construct the injection middleware
//!
//! # Quick Start
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
//!         .rate(0.1) // 10% of requests get injected output
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
//! The [`InjectionLayer`] uses a type-state pattern to enforce that all required
//! properties are configured before the layer can be built. This compile-time
//! safety ensures that you cannot accidentally create an injection layer without
//! properly specifying the rate and the output factory:
//!
//! - [`rate`][InjectionLayer::rate]: Required probability of injection in `[0.0, 1.0]`
//! - [`output_with`][InjectionLayer::output_with], [`output`][InjectionLayer::output],
//!   [`output_error_with`][InjectionLayer::output_error_with], or
//!   [`output_error`][InjectionLayer::output_error]:
//!   Required factory that produces the injected output
//!
//! Each injection layer requires an identifier for telemetry purposes. This
//! identifier should use `snake_case` naming convention to maintain consistency
//! across the codebase.
//!
//! # Output Factory
//!
//! The injected output can be produced in two ways:
//!
//! - [`output_with`][InjectionLayer::output_with]: Callback - the closure receives
//!   the consumed input and [`InjectionOutputArgs`], and returns the output.
//! - [`output`][InjectionLayer::output]: Convenience shorthand that clones a fixed
//!   value on every invocation.
//! - [`output_error_with`][InjectionLayer::output_error_with]: Like `output_with`, but
//!   the closure returns only the error value, which is automatically wrapped in `Err`.
//! - [`output_error`][InjectionLayer::output_error]: Like `output`, but takes an error
//!   value that is cloned and wrapped in `Err` on every invocation.
//!
//! # Defaults
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Rate | `None` (required) | Probability of injection | [`rate`][InjectionLayer::rate] |
//! | Output | `None` (required) | Produces the injected output | [`output_with`][InjectionLayer::output_with], [`output`][InjectionLayer::output], [`output_error_with`][InjectionLayer::output_error_with], [`output_error`][InjectionLayer::output_error] |
//! | Enable condition | Always enabled | Injection is applied to all requests | [`enable_if`][InjectionLayer::enable_if], [`enable_always`][InjectionLayer::enable_always], [`disable`][InjectionLayer::disable] |
//!
//! # Thread Safety
//!
//! The [`Injection`] type is thread-safe and implements both `Send` and `Sync` as
//! enforced by the `Service` trait it implements.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted when an injection is triggered
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Injection identifier from [`Injection::layer`]
//!   - `resilience.event.name`: Always `chaos_injection`
//!
//! # Examples
//!
//! ## Basic Usage
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
//!         .rate(0.05) // 5% injection rate
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
//! ## From Configuration
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
//! ## With Conditional Enable
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

pub use args::InjectionOutputArgs;
pub(crate) use callbacks::InjectionOutput;
pub use config::InjectionConfig;
pub use layer::InjectionLayer;
pub use service::Injection;
#[cfg(feature = "tower-service")]
pub use service::InjectionFuture;
pub(crate) use service::InjectionShared;
