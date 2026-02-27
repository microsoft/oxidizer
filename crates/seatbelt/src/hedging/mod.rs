// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hedging resilience middleware for reducing tail latency via additional concurrent execution.
//!
//! This module provides hedging capabilities that launch additional concurrent requests
//! to reduce the impact of slow responses. The primary types are [`Hedging`] and [`HedgingLayer`]:
//!
//! - [`Hedging`] is the middleware that wraps an inner service and launches parallel hedge requests
//! - [`HedgingLayer`] is used to configure and construct the hedging middleware
//!
//! # Quick Start
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::hedging::{Hedging, HedgingMode};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let context = ResilienceContext::new(&clock).name("my_service");
//!
//! let stack = (
//!     Hedging::layer("hedge", &context)
//!         .clone_input()
//!         .recovery_with(|result, _| match result {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         })
//!         .hedging_mode(HedgingMode::delay(Duration::from_secs(1))),
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
//! # How It Works
//!
//! Hedging sends the original request immediately. Based on the configured [`HedgingMode`]:
//!
//! - **Immediate**: All hedged requests launch at once
//! - **Delay**: Each hedge launches after a fixed delay if no acceptable result has arrived
//! - **Dynamic**: The delay is computed per hedge via a user-provided callback
//!
//! The first result classified as non-recoverable (via the recovery callback) is returned
//! immediately. Any remaining in-flight requests are cancelled.
//!
//! # Configuration
//!
//! The [`HedgingLayer`] uses a type state pattern to enforce that all required properties are
//! configured before the layer can be built. This compile-time safety ensures that you cannot
//! accidentally create a hedging layer without properly specifying input cloning and recovery logic:
//!
//! - [`clone_input_with`][HedgingLayer::clone_input_with]: Required function to clone inputs for
//!   hedged attempts (Rust ownership requirement)
//! - [`recovery`][HedgingLayer::recovery]: Required function to classify whether an output is
//!   acceptable
//!
//! Each hedging layer requires an identifier for telemetry purposes. This identifier should use
//! `snake_case` naming convention to maintain consistency across the codebase.
//!
//! # Thread Safety
//!
//! The [`Hedging`] type is thread-safe and implements both `Send` and `Sync` as enforced by
//! the `Service` trait it implements. This allows hedging middleware to be safely shared
//! across multiple threads and used in concurrent environments.
//!
//! # Defaults
//!
//! The hedging middleware uses the following default values when optional configuration is not
//! provided:
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Max hedged attempts | `1` (2 total) | Additional hedge requests beyond the original | [`max_hedged_attempts`][HedgingLayer::max_hedged_attempts] |
//! | Hedging mode | `delay(2s)` | Wait 2 seconds before each hedge | [`hedging_mode`][HedgingLayer::hedging_mode] |
//! | Handle unavailable | `false` | Unavailable responses are returned immediately | [`handle_unavailable`][HedgingLayer::handle_unavailable] |
//! | Enable condition | Always enabled | Hedging is applied to all requests | [`enable_if`][HedgingLayer::enable_if], [`enable_always`][HedgingLayer::enable_always], [`disable`][HedgingLayer::disable] |
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted for each hedged request launched
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Hedging identifier from [`Hedging::layer`]
//!   - `resilience.event.name`: Always `hedge`
//!   - `resilience.attempt.index`: Attempt index (1-based for hedges)
//!   - `resilience.attempt.is_last`: Whether this is the last hedge attempt
//!
//! # Examples
//!
//! ## Basic Usage
//!
//! This example demonstrates the basic usage of configuring and using hedging middleware.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::hedging::{Hedging, HedgingMode};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! let context = ResilienceContext::new(&clock).name("example");
//!
//! let stack = (
//!     Hedging::layer("my_hedge", &context)
//!         // Required: how to clone inputs for hedged attempts
//!         .clone_input()
//!         // Required: determine if we should keep waiting for hedges
//!         .recovery_with(|output: &Result<String, String>, _args| match output {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(msg) if msg.contains("transient") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(),
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("test input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```
//!
//! ## Advanced Usage
//!
//! This example demonstrates advanced usage of the hedging middleware, including custom
//! hedging modes, on-hedge callbacks, and dynamic delays.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Stack, Service};
//! # use seatbelt::hedging::{Hedging, HedgingMode};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Hedging::layer("advanced_hedge", &context)
//!         .clone_input()
//!         .recovery_with(|output: &Result<String, String>, _args| match output {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         })
//!         // Optional configuration
//!         .max_hedged_attempts(3)
//!         .hedging_mode(HedgingMode::dynamic(|args| {
//!             Duration::from_millis(100 * u64::from(args.attempt().index()))
//!         }))
//!         // Callback called just before each hedge is launched
//!         .on_hedge(|args| {
//!             println!("launching hedge attempt: {}", args.attempt());
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! let service = stack.into_service();
//! let result = service.execute("test input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```

mod args;
mod callbacks;
mod constants;
mod layer;
mod mode;
mod service;
#[cfg(any(feature = "metrics", test))]
mod telemetry;

pub use crate::attempt::Attempt;
pub use args::{CloneArgs, HedgingDelayArgs, OnHedgeArgs, RecoveryArgs};
pub use layer::HedgingLayer;
pub use mode::HedgingMode;
pub use service::Hedging;
#[cfg(feature = "tower-service")]
pub use service::HedgingFuture;
