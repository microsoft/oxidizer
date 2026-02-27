// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hedging resilience middleware for reducing tail latency via speculative execution.
//!
//! This module provides hedging capabilities that launch concurrent speculative requests
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
//!         .try_clone()
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
//! configured before the layer can be built:
//!
//! - [`try_clone`][HedgingLayer::try_clone]: Required function to clone inputs for hedged attempts
//! - [`recovery`][HedgingLayer::recovery]: Required function to classify whether an output is acceptable
//!
//! # Thread Safety
//!
//! The [`Hedging`] type is thread-safe and implements both `Send` and `Sync`.
//!
//! # Defaults
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Max hedged attempts | `1` (2 total) | Additional hedge requests beyond the original | [`max_hedged_attempts`][HedgingLayer::max_hedged_attempts] |
//! | Hedging mode | `delay(2s)` | Wait 2 seconds before each hedge | [`hedging_mode`][HedgingLayer::hedging_mode] |
//! | Enable condition | Always enabled | Hedging is applied to all requests | [`enable_if`][HedgingLayer::enable_if] |
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

mod args;
mod callbacks;
mod constants;
mod layer;
mod mode;
mod service;
#[cfg(any(feature = "metrics", test))]
mod telemetry;

pub use crate::attempt::Attempt;
pub use args::{HedgingDelayArgs, OnHedgeArgs, RecoveryArgs, TryCloneArgs};
pub use layer::HedgingLayer;
pub use mode::HedgingMode;
pub use service::Hedging;
#[cfg(feature = "tower-service")]
pub use service::HedgingFuture;
