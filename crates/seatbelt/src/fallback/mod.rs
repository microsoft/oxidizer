// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fallback resilience middleware for services, applications, and libraries.
//!
//! This module provides a fallback mechanism that replaces invalid service output
//! with a user-defined alternative. The primary types are [`Fallback`] and
//! [`FallbackLayer`]:
//!
//! - [`Fallback`] is the middleware that wraps an inner service and applies fallback logic
//! - [`FallbackLayer`] is used to configure and construct the fallback middleware
//!
//! # Quick Start
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
//! The [`FallbackLayer`] uses a type-state pattern to enforce that all required
//! properties are configured before the layer can be built. This compile-time
//! safety ensures that you cannot accidentally create a fallback layer without
//! properly specifying the predicate and the fallback action:
//!
//! - [`should_fallback`][FallbackLayer::should_fallback]: Required predicate that decides whether the output needs replacing
//! - [`fallback`][FallbackLayer::fallback] or [`fallback_async`][FallbackLayer::fallback_async]: Required action that produces the replacement output
//!
//! Each fallback layer requires an identifier for telemetry purposes. This
//! identifier should use `snake_case` naming convention to maintain consistency
//! across the codebase.
//!
//! # Sync and Async Fallback
//!
//! Fallback actions can be either synchronous or asynchronous:
//!
//! - [`fallback`][FallbackLayer::fallback]: Synchronous - the closure runs inline.
//! - [`fallback_async`][FallbackLayer::fallback_async]: Asynchronous - the closure
//!   returns a `Future` that is `.await`ed.
//! - [`fallback_output`][FallbackLayer::fallback_output]: Convenience shorthand that
//!   clones a fixed value on every invocation.
//!
//! Both [`fallback`][FallbackLayer::fallback] and [`fallback_async`][FallbackLayer::fallback_async]
//! receive the original (invalid) output as their argument and produce a replacement.
//!
//! # Defaults
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Predicate | `None` (required) | Decides when fallback is needed | [`should_fallback`][FallbackLayer::should_fallback] |
//! | Action | `None` (required) | Produces the replacement output | [`fallback`][FallbackLayer::fallback], [`fallback_async`][FallbackLayer::fallback_async], [`fallback_output`][FallbackLayer::fallback_output] |
//! | Before-fallback callback | `None` | No observability by default | [`before_fallback`][FallbackLayer::before_fallback] |
//! | After-fallback callback | `None` | No observability by default | [`after_fallback`][FallbackLayer::after_fallback] |
//! | Enable condition | Always enabled | Fallback is applied to all requests | [`enable_if`][FallbackLayer::enable_if], [`enable_always`][FallbackLayer::enable_always], [`disable`][FallbackLayer::disable] |
//!
//! # Thread Safety
//!
//! The [`Fallback`] type is thread-safe and implements both `Send` and `Sync` as
//! enforced by the `Service` trait it implements.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted when a fallback action is invoked
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Fallback identifier from [`Fallback::layer`]
//!   - `resilience.event.name`: Always `fallback`
//!
//! # Examples
//!
//! ## Basic Usage
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
//!         .fallback(|_output: String, _args| "default_value".to_string()),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```
//!
//! ## Async Fallback
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
//!         .fallback_async(|_output: String, _args| async {
//!             "fetched_from_cache".to_string()
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("input".to_string()).await;
//! # }
//! # async fn execute_unreliable_operation(input: String) -> String { input }
//! ```
//!
//! ## With Observability
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
//!         .before_fallback(|original: &mut String, _args| {
//!             println!("fallback triggered, original output: {original}");
//!         })
//!         .after_fallback(|new_output: &mut String, _args| {
//!             println!("fallback complete, new output: {new_output}");
//!         })
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

pub use args::{AfterFallbackArgs, BeforeFallbackArgs, FallbackActionArgs};
pub(crate) use callbacks::{AfterFallback, BeforeFallback, FallbackAction, ShouldFallback};
pub use layer::FallbackLayer;
pub use service::Fallback;
#[cfg(feature = "tower-service")]
pub use service::FallbackFuture;
pub(crate) use service::FallbackShared;
