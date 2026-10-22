// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fallback middleware for services, applications, and libraries.
//!
//! This module replaces invalid service output with another value.
//! It provides these main types:
//!
//! - [`Fallback`] wraps an inner service and applies a fallback.
//! - [`FallbackLayer`] configures and creates the fallback middleware.
//!
//! # Quick start
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::fallback::Fallback;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock).name("my_service");
//!
//! let stack = (
//!     Fallback::layer("fallback", &context)
//!         .should_fallback(|output: &String| output == "bad")
//!         .fallback(|_output: String, _args| "replacement".to_string()),
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
//! [`FallbackLayer`] uses the type state pattern.
//! A type state records builder state in the type system.
//! The layer cannot build until you set these properties:
//!
//! - [`should_fallback`][FallbackLayer::should_fallback] decides whether to replace output.
//! - [`fallback`][FallbackLayer::fallback] or [`fallback_async`][FallbackLayer::fallback_async] creates replacement output.
//!
//! Each fallback layer requires an identifier for telemetry.
//! Use `snake_case` for this identifier.
//!
//! # Synchronous and asynchronous fallback
//!
//! A fallback action can be synchronous or asynchronous:
//!
//! - [`fallback`][FallbackLayer::fallback] runs the closure inline.
//! - [`fallback_async`][FallbackLayer::fallback_async] waits for the returned `Future`.
//! - [`fallback_output`][FallbackLayer::fallback_output] clones a fixed value.
//!
//! Both actions receive the invalid output and [`FallbackActionArgs`].
//!
//! # Defaults
//!
//! | Parameter | Default value | Description | Set by |
//! |-----------|---------------|-------------|---------------|
//! | Predicate | `None` (required) | Decides when fallback is needed | [`should_fallback`][FallbackLayer::should_fallback] |
//! | Action | `None` (required) | Produces the replacement output | [`fallback`][FallbackLayer::fallback], [`fallback_async`][FallbackLayer::fallback_async], [`fallback_output`][FallbackLayer::fallback_output] |
//! | Enable condition | Always enabled | Fallback is applied to all requests | [`enable_if`][FallbackLayer::enable_if], [`enable_always`][FallbackLayer::enable_always], [`disable`][FallbackLayer::disable] |
//!
//! # Thread safety
//!
//! [`Fallback`] implements `Send` and `Sync` as required by its `Service` implementation.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - Metric: `resilience.event` (counter)
//! - Emission: When a fallback action runs
//! - Attributes:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Fallback identifier from [`Fallback::layer`]
//!   - `resilience.event.name`: `fallback`
//!
//! # Examples
//!
//! ## Basic use
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::fallback::Fallback;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Fallback::layer("my_fallback", &context)
//!         .should_fallback(|output: &String| output.is_empty())
//!         .fallback_output("default_value".to_string()),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```
//!
//! ## Asynchronous fallback
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::fallback::Fallback;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Fallback::layer("my_fallback", &context)
//!         .should_fallback(|output: &String| output == "error")
//!         .fallback_async(|_output: String, _args| async { "fetched_from_cache".to_string() }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```
//!
//! ## Conditional fallback
//!
//! ```rust
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::ResilienceContext;
//! # use seatbelt::fallback::Fallback;
//! # async fn example(clock: Clock) {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Fallback::layer("my_fallback", &context)
//!         .should_fallback(|output: &String| output == "bad")
//!         .fallback(|_output: String, _args| "safe_default".to_string())
//!         .enable_if(|input: &String| !input.starts_with("bypass_")),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```

mod args;
mod callbacks;
mod layer;
mod service;

#[cfg(any(feature = "metrics", test))]
mod telemetry;

pub use args::FallbackActionArgs;
pub(crate) use callbacks::{FallbackAction, ShouldFallback};
pub use layer::FallbackLayer;
pub use service::Fallback;
#[cfg(feature = "tower-service")]
pub use service::FallbackFuture;
pub(crate) use service::FallbackShared;
