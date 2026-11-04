// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Latency injection middleware for services, applications, and libraries.
//!
//! This module adds a delay before the inner service call at a configured rate.
//! It provides these main types:
//!
//! - [`Latency`] wraps an inner service and adds delay.
//! - [`LatencyLayer`] configures and creates the latency middleware.
//!
//! # Quick start
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::latency::Latency;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock).name("my_service");
//!
//! let stack = (
//!     Latency::layer("latency", &context)
//!         .rate(0.1) // Delay 10% of requests.
//!         .latency(Duration::from_millis(200)),
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
//! [`LatencyLayer`] uses the type state pattern.
//! A type state records builder state in the type system.
//! The layer cannot build until you set these properties:
//!
//! - [`rate`][LatencyLayer::rate] or [`rate_with`][LatencyLayer::rate_with]:
//!   sets the injection rate in `[0.0, 1.0]`.
//! - [`latency`][LatencyLayer::latency], [`latency_with`][LatencyLayer::latency_with],
//!   or [`latency_range`][LatencyLayer::latency_range]:
//!   sets the delay.
//!
//! Each latency layer requires an identifier for telemetry.
//! Use `snake_case` for this identifier.
//!
//! # Latency duration
//!
//! The injected delay can be configured in three ways:
//!
//! - [`latency`][LatencyLayer::latency] uses a fixed duration.
//! - [`latency_with`][LatencyLayer::latency_with] calculates the delay from the input.
//! - [`latency_range`][LatencyLayer::latency_range] selects a uniform random duration.
//!
//! # Defaults
//!
//! | Parameter | Default value | Description | Set by |
//! |-----------|---------------|-------------|---------------|
//! | Rate | `None` (required) | Probability of latency injection | [`rate`][LatencyLayer::rate], [`rate_with`][LatencyLayer::rate_with] |
//! | Latency | `None` (required) | Delay duration to inject | [`latency`][LatencyLayer::latency], [`latency_with`][LatencyLayer::latency_with], [`latency_range`][LatencyLayer::latency_range] |
//! | Enable condition | Always enabled | Latency is applied to all requests | [`enable_if`][LatencyLayer::enable_if], [`enable`][LatencyLayer::enable] |
//!
//! # Thread safety
//!
//! [`Latency`] implements `Send` and `Sync` as required by its `Service` implementation.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - Metric: `resilience.event` (counter)
//! - Emission: When the middleware adds latency
//! - Attributes:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Latency identifier from [`Latency::layer`]
//!   - `resilience.event.name`: `chaos_latency`
//!
//! # Examples
//!
//! ## Basic use
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::latency::Latency;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Latency::layer("my_latency", &context)
//!         .rate(0.05) // Delay 5% of requests.
//!         .latency(Duration::from_millis(100)),
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
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::latency::{Latency, LatencyConfig};
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//! let mut config = LatencyConfig::default();
//! config.rate = 0.1;
//! config.latency = Duration::from_millis(200);
//!
//! let stack = (
//!     Latency::layer("my_latency", &context).config(&config),
//!     Execute::new(execute_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_operation(input: String) -> String { input }
//! ```
//!
//! ## Random range
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::latency::Latency;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Latency::layer("my_latency", &context)
//!         .rate(0.2)
//!         .latency_range(Duration::from_millis(100)..Duration::from_millis(500)),
//!     Execute::new(execute_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_operation(input: String) -> String { input }
//! ```
//!
//! ## Conditional latency
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::chaos::latency::Latency;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Latency::layer("my_latency", &context)
//!         .rate(0.2)
//!         .latency(Duration::from_millis(200))
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

pub use args::{LatencyDurationArgs, LatencyRateArgs};
pub(crate) use callbacks::{LatencyDuration, LatencyRate};
pub use config::LatencyConfig;
pub use layer::LatencyLayer;
pub use service::Latency;
#[cfg(feature = "tower-service")]
pub use service::LatencyFuture;
pub(crate) use service::LatencyShared;
