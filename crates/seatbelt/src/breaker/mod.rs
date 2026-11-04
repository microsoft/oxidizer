// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Circuit breaker middleware that limits repeated calls to a failing service.
//!
//! A circuit breaker blocks calls after too many failures.
//! You can configure the failure threshold, break duration, and telemetry.
//! The main types are:
//!
//! - [`Breaker`] wraps an inner service and monitors its failure rate.
//! - [`BreakerLayer`] configures and creates the circuit breaker.
//!
//! The breaker temporarily blocks input when failures reach the configured threshold.
//! This gives the downstream service time to recover.
//!
//! # Quick start
//!
//! ```rust
//! # use layered::{Execute, Service, Stack};
//! # use tick::Clock;
//! # use seatbelt::breaker::Breaker;
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let context = ResilienceContext::new(&clock);
//!
//! let stack = (
//!     Breaker::layer("breaker", &context)
//!         // Use recovery data to decide whether the output is a failure.
//!         .recovery_with(|result: &Result<String, String>, _| match result {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         })
//!         // Provide output when an open circuit rejects input.
//!         .rejected_input_error(|input, args| "service unavailable".to_string()),
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
//! # Configuration
//!
//! [`BreakerLayer`] uses the type state pattern.
//! A type state records builder state in the type system.
//! The layer cannot build until you set these properties:
//!
//! - [`recovery`][BreakerLayer::recovery] classifies output as success or failure.
//! - [`rejected_input`][BreakerLayer::rejected_input] provides output when an open circuit rejects input.
//!
//! Each circuit breaker layer requires an identifier for telemetry.
//! Use `snake_case` for this identifier.
//!
//! # Thread safety
//!
//! [`Breaker`] implements `Send` and `Sync` as required by its `Service` implementation.
//! You can share the middleware across threads.
//!
//! # Circuit breaker states and transitions
//!
//! The circuit breaker operates in three states:
//!
//! - Closed: Input reaches the service, and the breaker records results.
//! - Open: The breaker rejects input without calling the service.
//! - Half-open: The breaker sends a limited number of probes to test the service.
//!
//! ```text
//! ┌────────┐       Failure threshold reached      ┌──────────┐
//! │ Closed │ ────────────────────────────────────▶│   Open   │
//! └────────┘                                      └──────────┘
//!      ▲                                                 │
//!      │                                                 │
//!      │            ┌────────────────┐                   │
//!      └────────────│   Half-Open    │◀──────────────────┘
//!      Probing      └────────────────┘      Break duration
//!      successful                           elapsed
//! ```
//!
//! ## Closed state
//!
//! The circuit starts in the closed state and operates normally:
//!
//! - All input reaches the service.
//! - The breaker compares the failure rate with the threshold.
//! - The circuit opens when failures reach the threshold.
//! - [`on_closed`][BreakerLayer::on_closed] reports a transition to closed.
//!
//! ## Open state
//!
//! When the circuit is open:
//!
//! - [`rejected_input`][BreakerLayer::rejected_input] provides output for rejected input.
//! - The breaker does not call the service.
//! - The circuit becomes half-open after the break duration.
//! - [`on_opened`][BreakerLayer::on_opened] reports a transition to open.
//!
//! ## Half-open state
//!
//! The circuit tests the service:
//!
//! - A limited number of probes reach the service.
//! - Successful probes can close the circuit.
//! - Failed probes keep the circuit half-open until the service recovers.
//! - The breaker waits for the break duration before it permits more probes.
//! - [`on_probing`][BreakerLayer::on_probing] reports each probe.
//! - [`half_open_mode`][BreakerLayer::half_open_mode] configures probe behavior.
//!
//! # Recovery classification
//!
//! The circuit breaker uses [`RecoveryInfo`][crate::RecoveryInfo] to classify output.
//! These recovery kinds count as failures:
//!
//! - [`RecoveryKind::Retry`][crate::RecoveryKind::Retry]
//! - [`RecoveryKind::Unavailable`][crate::RecoveryKind::Unavailable]
//!
//! # Abandoned executions
//!
//! An execution is abandoned when its future is dropped before it finishes.
//! For example, the caller can cancel the request.
//! Telemetry includes abandoned executions in throughput.
//! [`AbandonedPolicy`] controls how they affect circuit health.
//! Set it with [`abandoned_policy`][BreakerLayer::abandoned_policy].
//!
//! The same policy applies both to the closed-state health window and to the half-open probes.
//!
//! # Isolated circuit breaker instances
//!
//! [`breaker_id`][BreakerLayer::breaker_id] can give input groups separate circuit state.
//! For HTTP requests, derive the ID from the scheme, host, and port.
//!
//! See [`BreakerId`] for rules about breaker IDs.
//!
//! # Defaults
//!
//! The circuit breaker uses these values when you do not provide optional configuration:
//!
//! | Parameter | Default value | Description | Set by |
//! |-----------|---------------|-------------|---------------|
//! | Failure threshold | `0.1` (10%) | Circuit opens when failure rate reaches this percentage | [`failure_threshold`][BreakerLayer::failure_threshold] |
//! | Minimum throughput | `100` executions | Minimum number of results before the circuit can open | [`min_throughput`][BreakerLayer::min_throughput] |
//! | Sampling duration | `30` seconds | Time window for calculating failure rates | [`sampling_duration`][BreakerLayer::sampling_duration] |
//! | Break duration | `5` seconds | Duration circuit remains open before testing recovery | [`break_duration`][BreakerLayer::break_duration] |
//! | Breaker isolation | Shared circuit (default) | All inputs share the same circuit breaker state | [`breaker_id`][BreakerLayer::breaker_id] |
//! | Half-open mode | `Progressive` | Increases the probe rate in stages | [`half_open_mode`][BreakerLayer::half_open_mode] |
//! | Abandoned policy | `rate_threshold(1.0)` | How abandoned (cancelled) executions affect the health decision | [`abandoned_policy`][BreakerLayer::abandoned_policy] |
//! | Enable condition | Always enabled | Circuit breaker protection is applied to all inputs | [`enable_if`][BreakerLayer::enable_if], [`enable_always`][BreakerLayer::enable_always], [`disable`][BreakerLayer::disable] |
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - Metric: `resilience.event` (counter)
//! - Emission: When state changes, input is rejected, or a probe enters
//! - Attributes:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Circuit breaker identifier from [`Breaker::layer`]
//!   - `resilience.event.name`: One of:
//!     - `circuit_opened`: When the circuit opens because the failure rate reached the threshold
//!     - `circuit_closed`: When the circuit transitions to closed state after successful probing
//!     - `circuit_rejected`: When an input is rejected due to the circuit being in open state
//!     - `circuit_probe`: When a probe is executed in half-open state
//!   - `resilience.circuit_breaker.id`: The breaker ID identifying the circuit breaker instance
//!   - `resilience.circuit_breaker.state`: Current circuit state (`closed`, `open`, or `half_open`)
//!   - `resilience.circuit_breaker.probe.result`: Result of probe execution (`success` or `failure`, only present for probe events)
//!
//! Structured logs include total and abandoned executions for state changes.
//!
//! # Examples
//!
//! ## Basic use
//!
//! This example configures and uses circuit breaker middleware.
//!
//! ```rust
//! # use layered::{Execute, Service, Stack};
//! # use tick::Clock;
//! # use seatbelt::breaker::Breaker;
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! // Create shared middleware configuration with a clock for the runtime.
//! let context = ResilienceContext::new(&clock).name("example");
//!
//! let stack = (
//!     Breaker::layer("my_breaker", &context)
//!         // Decide whether the output is a failure.
//!         .recovery_with(|result: &Result<String, String>, _args| match result {
//!             // Replace these example rules with rules for the service.
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(msg) if msg.contains("transient") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(),
//!         })
//!         // Provide output while the circuit is open.
//!         .rejected_input_error(|_, _| "service unavailable".to_string()),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build the service.
//! let service = stack.into_service();
//!
//! // Execute the service.
//! let result = service.execute("test input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```
//!
//! ## Advanced use
//!
//! This example sets thresholds, durations, and state callbacks.
//!
//! ```rust
//! # use std::time::Duration;
//! # use layered::{Execute, Service, Stack};
//! # use tick::Clock;
//! # use seatbelt::breaker::{Breaker, BreakerId, HalfOpenMode};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! // Create shared middleware configuration.
//! let context = ResilienceContext::new(&clock).name("advanced_example");
//!
//! let stack = (
//!     Breaker::layer("advanced_breaker", &context)
//!         // Decide whether the output is a failure.
//!         .recovery_with(|result: &Result<String, String>, _args| match result {
//!             Err(msg) if msg.contains("rate_limit") => RecoveryInfo::unavailable(),
//!             Err(msg) if msg.contains("timeout") => RecoveryInfo::retry(),
//!             Err(msg) if msg.contains("server_error") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(), // Do not count client errors as failures.
//!             Ok(_) => RecoveryInfo::never(),
//!         })
//!         // Provide output while the circuit is open.
//!         .rejected_input_error(|_input, _args| {
//!             "service temporarily unavailable because the failure rate reached the threshold".to_string()
//!         })
//!         // Set optional configuration.
//!         .half_open_mode(HalfOpenMode::progressive(None)) // close the circuit gradually (default)
//!         .failure_threshold(0.05) // Trip at 5% failure threshold (less sensitive than default 10%)
//!         .min_throughput(50)  // Require minimum 50 executions before considering circuit open
//!         .sampling_duration(Duration::from_secs(60)) // Evaluate failures over 60-second window
//!         .break_duration(Duration::from_secs(30))    // Stay open for 30 seconds before testing
//!         // You can provide your own breaker ID logic if needed. By default, all inputs share the
//!         // same circuit. By providing distinct IDs, you can partition into separate circuits.
//!         .breaker_id(|input| BreakerId::from(detect_breaker_id(input)))
//!         // State change callbacks for monitoring and alerting
//!         .on_opened(|output, _args| {
//!             println!("circuit breaker OPENED due to failure: {:?}", output);
//!             // Production code can trigger alerts, metrics, or logs here.
//!         })
//!         .on_closed(|output, _args| {
//!             println!("circuit breaker CLOSED, service recovered: {:?}", output);
//!             // Production code can log recovery or update dashboards here.
//!         })
//!         .on_probing(|input, _args| {
//!             println!("circuit breaker PROBING with input: {:?}", input);
//!             // Optionally modify input for probes
//!         }),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build and execute the service
//! let service = stack.into_service();
//! let result = service.execute("test_timeout".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # fn detect_breaker_id(input: &String) -> String  { input.to_string() }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```

mod args;
mod callbacks;
mod config;
mod layer;
mod service;
#[doc(inline)]
pub use args::{OnClosedArgs, OnOpenedArgs, OnProbingArgs, RecoveryArgs, RejectedInputArgs};
pub(super) use callbacks::*;
pub use config::BreakerConfig;
#[doc(inline)]
pub use layer::BreakerLayer;
#[doc(inline)]
pub use service::Breaker;
#[cfg(feature = "tower-service")]
pub use service::BreakerFuture;
pub(crate) use service::BreakerShared;

mod execution_result;
pub(super) use execution_result::ExecutionResult;

mod health;
pub(super) use health::*;

mod health_eval;
pub(super) use health_eval::HealthEvaluator;

mod constants;
mod engine;

#[cfg(any(feature = "metrics", test))]
mod telemetry;
pub(super) use engine::*;

mod breaker_id;
pub use breaker_id::BreakerId;

mod half_open_mode;
pub use half_open_mode::HalfOpenMode;

mod abandoned_policy;
pub use abandoned_policy::AbandonedPolicy;
