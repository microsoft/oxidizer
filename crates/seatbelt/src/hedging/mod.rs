// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hedging middleware that starts concurrent attempts to reduce tail latency.
//!
//! Hedging starts more requests when earlier requests are slow.
//! It provides these main types:
//!
//! - [`Hedging`] wraps an inner service and starts concurrent attempts.
//! - [`HedgingLayer`] configures and creates the hedging middleware.
//!
//! # Quick start
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::hedging::Hedging;
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let context = ResilienceContext::new(&clock).name("my_service");
//!
//! let stack = (
//!     Hedging::layer("hedging", &context)
//!         .clone_input()
//!         .recovery_with(|result, _| match result {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         })
//!         .hedging_delay(Duration::from_secs(1)),
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
//! # How it works
//!
//! Hedging sends the original request immediately. Based on the configured delay:
//!
//! - Zero delay: All attempts start at once with `hedging_delay(Duration::ZERO)`.
//! - Fixed delay: Each attempt starts after a fixed delay if no acceptable result arrives.
//!   Use [`hedging_delay`][HedgingLayer::hedging_delay].
//! - Dynamic delay: A callback calculates the delay for each attempt from the input.
//!   Use
//!   [`hedging_delay_with`][HedgingLayer::hedging_delay_with].
//!
//! The first result classified as non-recoverable (via the recovery callback) is returned
//! immediately. Any remaining in-flight requests are cancelled.
//! [`RecoveryKind::Unknown`][crate::RecoveryKind::Unknown] is treated as non-recoverable
//! and accepted immediately.
//!
//! # When to use hedging
//!
//! Hedging adds load because it sends concurrent requests.
//! The downstream service must have capacity for this load.
//! Do not use hedging when the service is already under pressure.
//!
//! If the service cannot accept more load, use [`retry`][crate::retry].
//! Retry starts a new request only after the previous request finishes.
//!
//! # Configuration
//!
//! [`HedgingLayer`] uses the type state pattern.
//! A type state records builder state in the type system.
//! The layer cannot build until you set these properties:
//!
//! - [`clone_input_with`][HedgingLayer::clone_input_with] provides input for hedged attempts.
//! - [`recovery`][HedgingLayer::recovery] classifies output.
//!
//! Each hedging layer requires an identifier for telemetry.
//! Use `snake_case` for this identifier.
//!
//! # Thread safety
//!
//! [`Hedging`] implements `Send` and `Sync` as required by its `Service` implementation.
//! You can share the middleware across threads.
//!
//! # Defaults
//!
//! Hedging uses these values when you do not provide optional configuration:
//!
//! | Parameter | Default value | Description | Set by |
//! |-----------|---------------|-------------|---------------|
//! | Maximum hedged attempts | `1` (2 total) | Additional attempts after the original | [`max_hedged_attempts`][HedgingLayer::max_hedged_attempts] |
//! | Hedging delay | `500ms` | Wait 500 milliseconds before each hedging attempt | [`hedging_delay`][HedgingLayer::hedging_delay], [`hedging_delay_with`][HedgingLayer::hedging_delay_with] |
//! | Handle unavailable | `false` | Unavailable responses are returned immediately | [`handle_unavailable`][HedgingLayer::handle_unavailable] |
//! | Enable condition | Always enabled | Hedging is applied to all requests | [`enable_if`][HedgingLayer::enable_if], [`enable_always`][HedgingLayer::enable_always], [`disable`][HedgingLayer::disable] |
//!
//! # Telemetry
//!
//! Hedging emits telemetry when an in-flight future is dropped.
//! Each attempt has a telemetry guard that reports the drop.
//!
//! Telemetry is emitted when:
//! - A result is recoverable. It reports the [`RecoveryKind`][crate::RecoveryKind].
//! - A future is abandoned. It reports the recovery kind `"abandoned"`.
//!
//! Telemetry is not emitted for accepted results.
//!
//! ## Metrics
//!
//! - Metric: `resilience.event` (counter)
//! - Emission: For recoverable and abandoned attempts
//! - Attributes:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Hedging identifier from [`Hedging::layer`]
//!   - `resilience.event.name`: `hedging`
//!   - `resilience.attempt.index`: Attempt index (0 for original, 1 and more for hedging attempts)
//!   - `resilience.attempt.is_last`: Whether this is the last attempt
//!   - `resilience.attempt.recovery.kind`: The recovery classification (`retry`, `unavailable`, or `abandoned`)
//!
//! ## Logs
//!
//! Log events include all metric attributes plus `resilience.hedging.delay` (the delay
//! in seconds waited before launching the attempt).
//!
//! # Examples
//!
//! ## Basic use
//!
//! This example configures and uses hedging middleware.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Service, Stack};
//! # use seatbelt::hedging::Hedging;
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! let context = ResilienceContext::new(&clock).name("example");
//!
//! let stack = (
//!     Hedging::layer("my_hedging", &context)
//!         // Clone input for each hedged attempt.
//!         .clone_input()
//!         // Decide whether to wait for another attempt.
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
//! ## Advanced use
//!
//! This example uses dynamic delays and execution callbacks.
//!
//! ```rust
//! # use std::time::Duration;
//! # use tick::Clock;
//! # use layered::{Execute, Stack, Service};
//! # use seatbelt::hedging::Hedging;
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), String> {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Hedging::layer("advanced_hedging", &context)
//!         .clone_input()
//!         .recovery_with(|output: &Result<String, String>, _args| match output {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         })
//!         // Set optional configuration.
//!         .max_hedged_attempts(3)
//!         .hedging_delay_with(|_input, args| {
//!             Duration::from_millis(100 * u64::from(args.attempt().index()))
//!         })
//!         // Run a callback before each operation.
//!         .on_execute(|_input, args| {
//!             println!("launching attempt: {}", args.attempt());
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
mod config;
mod constants;
mod layer;
mod service;
mod telemetry;

pub use args::{CloneArgs, HedgingDelayArgs, OnExecuteArgs, RecoveryArgs};
pub use config::HedgingConfig;
pub use layer::HedgingLayer;
pub use service::Hedging;
#[cfg(feature = "tower-service")]
pub use service::HedgingFuture;

#[deprecated(note = "import from `seatbelt::Attempt` instead")]
pub use crate::attempt::Attempt;
