// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Chaos latency middleware for services, applications, and libraries.
//!
//! This module provides a latency injection mechanism that adds an artificial
//! delay before the inner service call at a configurable probability. The
//! primary types are [`Latency`] and [`LatencyLayer`]:
//!
//! - [`Latency`] is the middleware that wraps an inner service and injects delay
//! - [`LatencyLayer`] is used to configure and construct the latency middleware
//!
//! # Quick Start
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
//!         .rate(0.1) // 10% of requests get delayed
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
//! The [`LatencyLayer`] uses a type-state pattern to enforce that all required
//! properties are configured before the layer can be built. This compile-time
//! safety ensures that you cannot accidentally create a latency layer without
//! properly specifying the rate and the delay duration:
//!
//! - [`rate`][LatencyLayer::rate] or [`rate_with`][LatencyLayer::rate_with]:
//!   Required probability of latency injection in `[0.0, 1.0]`
//! - [`latency`][LatencyLayer::latency], [`latency_with`][LatencyLayer::latency_with],
//!   or [`latency_range`][LatencyLayer::latency_range]:
//!   Required delay duration
//!
//! Each latency layer requires an identifier for telemetry purposes. This
//! identifier should use `snake_case` naming convention to maintain consistency
//! across the codebase.
//!
//! # Latency Duration
//!
//! The injected delay can be configured in three ways:
//!
//! - [`latency`][LatencyLayer::latency]: Fixed duration applied on every injection.
//! - [`latency_with`][LatencyLayer::latency_with]: Callback - the closure receives
//!   a reference to the input and [`LatencyDurationArgs`], and returns the delay.
//! - [`latency_range`][LatencyLayer::latency_range]: Uniform random duration chosen
//!   from a [`Range<Duration>`][std::ops::Range].
//!
//! # Defaults
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Rate | `None` (required) | Probability of latency injection | [`rate`][LatencyLayer::rate], [`rate_with`][LatencyLayer::rate_with] |
//! | Latency | `None` (required) | Delay duration to inject | [`latency`][LatencyLayer::latency], [`latency_with`][LatencyLayer::latency_with], [`latency_range`][LatencyLayer::latency_range] |
//! | Enable condition | Always enabled | Latency is applied to all requests | [`enable_if`][LatencyLayer::enable_if], [`enable`][LatencyLayer::enable] |
//!
//! # Thread Safety
//!
//! The [`Latency`] type is thread-safe and implements both `Send` and `Sync` as
//! enforced by the `Service` trait it implements.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted when latency is injected
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Latency identifier from [`Latency::layer`]
//!   - `resilience.event.name`: Always `chaos_latency`
//!
//! # Examples
//!
//! ## Basic Usage
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
//!         .rate(0.05) // 5% injection rate
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
//! ## From Configuration
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
//!     Latency::layer("my_latency", &context)
//!         .config(&config),
//!     Execute::new(execute_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_operation(input: String) -> String { input }
//! ```
//!
//! ## With Random Range
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
//! ## With Conditional Enable
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
