// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Circuit breaker resilience middleware for preventing cascading failures.
//!
//! This module provides automatic circuit breaking capabilities with configurable failure
//! thresholds, break duration, and comprehensive telemetry. The primary types are:
//!
//! - [`Breaker`] is the middleware that wraps an inner service and monitors failure rates
//! - [`BreakerLayer`] is used to configure and construct the circuit breaker middleware
//!
//! A circuit breaker monitors the success and failure rates of operations and can temporarily
//! block inputs when the failure rate exceeds a configured threshold. This prevents cascading failures
//! and gives downstream services time to recover.
//!
//! # Quick Start
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
//!         // Required: determine if output indicates a failure by using recovery metadata
//!         .recovery_with(|result: &Result<String, String>, _| match result {
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(_) => RecoveryInfo::retry(),
//!         })
//!         // Required: provide output when the input is rejected on an open circuit
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
//! The [`BreakerLayer`] uses a type state pattern to enforce that all required properties
//! are configured before the layer can be built. This compile-time safety ensures that you cannot
//! accidentally create a circuit breaker layer without properly specifying recovery logic and
//! rejected input handling. The properties that must be configured are:
//!
//! - [`recovery`][BreakerLayer::recovery]: Detects the recovery classification for output.
//!   This is used to determine if an operation succeeded or failed.
//! - [`rejected_input`][BreakerLayer::rejected_input]: Provide the output to return when the
//!   circuit is open and execution is being rejected.
//!
//! Each circuit breaker layer requires an identifier for telemetry purposes. This identifier
//! should use `snake_case` naming convention to maintain consistency across the telemetry.
//!
//! # Thread Safety
//!
//! The [`Breaker`] type is thread-safe and implements both `Send` and `Sync` as enforced
//! by the `Service` trait it implements. This allows circuit breaker middleware to be safely
//! shared across multiple threads and used in concurrent environments.
//!
//! # Circuit Breaker States and Transitions
//!
//! The circuit breaker operates in three states:
//!
//! - **Closed**: Normal operation. Inputs pass through and failures are tracked.
//! - **Open**: The circuit is broken. Inputs are immediately rejected without calling
//!   the underlying service.
//! - **Half-Open**: Testing if the service has recovered. A limited number of probe inputs are
//!   allowed through to assess the health of the underlying service.
//!
//! ```text
//! ┌────────┐      Failure threshold exceeded      ┌──────────┐
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
//! ## Closed State
//!
//! The circuit starts in the closed state and operates normally:
//!
//! - All inputs pass through to the underlying service
//! - Failures are tracked and evaluated against the failure threshold
//! - When the failure threshold is exceeded, transitions to **Open**
//! - You can observe transitions into the closed state by providing
//!   the [`on_closed`][BreakerLayer::on_closed] callback.
//!
//! ## Open State
//!
//! When the circuit is open:
//!
//! - Inputs are immediately rejected with the output provided by [`rejected_input`][BreakerLayer::rejected_input]
//! - No calls are made to the underlying service
//! - After the break duration elapsed, transitions to **Half-Open**
//! - You can observe transitions into the open state by providing
//!   the [`on_opened`][BreakerLayer::on_opened] callback.
//!
//! ## Half-Open State
//!
//! The circuit enters a testing phase:
//!
//! - A limited number of probe inputs are allowed through
//! - Success rate is carefully monitored
//! - If sufficient successful probes occur, transitions back to **Closed**
//! - If failures continue, the circuit stays in the Half-Open state until the underlying service recovers.
//!   Half-open state respects the break duration before allowing more probes.
//! - You can observe when the circuit is probing in half-open state by providing
//!   the [`on_probing`][BreakerLayer::on_probing] callback.
//! - You can configure the probing behavior and the sensitivity of how quickly the circuit
//!   closes again after successful probes by using [`half_open_mode`][BreakerLayer::half_open_mode]
//!
//! # Recovery Classification
//!
//! The circuit breaker uses [`RecoveryInfo`][crate::RecoveryInfo] to classify operation results. The following
//! recovery kinds are classified as failures that contribute to tripping the circuit:
//!
//! - [`RecoveryKind::Retry`][crate::RecoveryKind::Retry]
//! - [`RecoveryKind::Unavailable`][crate::RecoveryKind::Unavailable]
//!
//! # Isolated Circuit Breaker Instances
//!
//! Circuit breakers can maintain separate states for different logical groups of inputs
//! by providing a [`breaker_id`][BreakerLayer::breaker_id] function. A typical use case is
//! HTTP requests where the breaker ID is derived from scheme, host, and port to isolate
//! circuit states for different downstream endpoints.
//!
//! See [`BreakerId`] for guidance on choosing appropriate IDs.
//!
//! # Defaults
//!
//! The circuit breaker middleware uses the following default values when optional configuration
//! is not provided:
//!
//! | Parameter | Default Value | Description | Configured By |
//! |-----------|---------------|-------------|---------------|
//! | Failure threshold | `0.1` (10%) | Circuit opens when failure rate exceeds this percentage | [`failure_threshold`][BreakerLayer::failure_threshold] |
//! | Minimum throughput | `100` executions | Minimum volume required before circuit can open | [`min_throughput`][BreakerLayer::min_throughput] |
//! | Sampling duration | `30` seconds | Time window for calculating failure rates | [`sampling_duration`][BreakerLayer::sampling_duration] |
//! | Break duration | `5` seconds | Duration circuit remains open before testing recovery | [`break_duration`][BreakerLayer::break_duration] |
//! | Breaker isolation | Single global circuit | All inputs share the same circuit breaker state | [`breaker_id`][BreakerLayer::breaker_id] |
//! | Half-open mode | `Reliable` | Gradual recovery with increasing probe percentages | [`half_open_mode`][BreakerLayer::half_open_mode] |
//! | Enable condition | Always enabled | Circuit breaker protection is applied to all inputs | [`enable_if`][BreakerLayer::enable_if], [`enable_always`][BreakerLayer::enable_always], [`disable`][BreakerLayer::disable] |
//!
//! These defaults provide a reasonable starting point for most use cases, offering a balance
//! between resilience and responsiveness to service recovery.
//!
//! # Telemetry
//!
//! ## Metrics
//!
//! - **Metric**: `resilience.event` (counter)
//! - **When**: Emitted when circuit state transitions occur and when inputs are rejected
//! - **Attributes**:
//!   - `resilience.pipeline.name`: Pipeline identifier from [`ResilienceContext::name`][crate::ResilienceContext::name]
//!   - `resilience.strategy.name`: Circuit breaker identifier from [`Breaker::layer`]
//!   - `resilience.event.name`: One of:
//!     - `circuit_opened`: When the circuit transitions to open state due to failure threshold being exceeded
//!     - `circuit_closed`: When the circuit transitions to closed state after successful probing
//!     - `circuit_rejected`: When an input is rejected due to the circuit being in open state
//!     - `circuit_probe`: When a probe is executed in half-open state
//!   - `resilience.circuit_breaker.id`: The breaker ID identifying the circuit breaker instance
//!   - `resilience.circuit_breaker.state`: Current circuit state (`closed`, `open`, or `half_open`)
//!   - `resilience.circuit_breaker.probe.result`: Result of probe execution (`success` or `failure`, only present for probe events)
//!
//! Additional structured logging events are emitted with detailed health metrics (failure rate, throughput) for circuit state transitions.
//!
//! # Examples
//!
//! ## Basic Usage
//!
//! This example demonstrates the basic usage of configuring and using circuit breaker middleware.
//!
//! ```rust
//! # use layered::{Execute, Service, Stack};
//! # use tick::Clock;
//! # use seatbelt::breaker::Breaker;
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! // Define common options for resilience middleware. The clock is runtime-specific and
//! // must be provided. See its documentation for details.
//! let context = ResilienceContext::new(&clock).name("example");
//!
//! let stack = (
//!     Breaker::layer("my_breaker", &context)
//!         // Required: determine if output indicates failure
//!         .recovery_with(|result: &Result<String, String>, _args| match result {
//!             // These are demonstrative, real code will have more meaningful recovery detection
//!             Ok(_) => RecoveryInfo::never(),
//!             Err(msg) if msg.contains("transient") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(),
//!         })
//!         // Required: provide output when circuit is open
//!         .rejected_input_error(|_, _| "service unavailable".to_string()),
//!     Execute::new(execute_unreliable_operation),
//! );
//!
//! // Build the service
//! let service = stack.into_service();
//!
//! // Execute the service
//! let result = service.execute("test input".to_string()).await;
//! # let _result = result;
//! # Ok(())
//! # }
//! # async fn execute_unreliable_operation(input: String) -> Result<String, String> { Ok(input) }
//! ```
//!
//! ## Advanced Usage
//!
//! This example demonstrates advanced usage of the circuit breaker middleware, including custom
//! failure thresholds, sampling duration, break duration, and state change callbacks.
//!
//! ```rust
//! # use std::time::Duration;
//! # use layered::{Execute, Service, Stack};
//! # use tick::Clock;
//! # use seatbelt::breaker::{Breaker, BreakerId, HalfOpenMode};
//! # use seatbelt::{RecoveryInfo, ResilienceContext};
//! # async fn example(clock: Clock) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! // Define common options for resilience middleware.
//! let context = ResilienceContext::new(&clock).name("advanced_example");
//!
//! let stack = (
//!     Breaker::layer("advanced_breaker", &context)
//!         // Required: determine if output indicates failure
//!         .recovery_with(|result: &Result<String, String>, _args| match result {
//!             Err(msg) if msg.contains("rate_limit") => RecoveryInfo::unavailable(),
//!             Err(msg) if msg.contains("timeout") => RecoveryInfo::retry(),
//!             Err(msg) if msg.contains("server_error") => RecoveryInfo::retry(),
//!             Err(_) => RecoveryInfo::never(), // Client errors don't count as failures
//!             Ok(_) => RecoveryInfo::never(),
//!         })
//!         // Required: provide output when circuit is open
//!         .rejected_input_error(|_input, _args| {
//!             "service temporarily unavailable due to exceeding failure threshold".to_string()
//!         })
//!         // Optional configuration
//!         .half_open_mode(HalfOpenMode::reliable(None)) // close the circuit gradually (default)
//!         .failure_threshold(0.05) // Trip at 5% failure threshold (less sensitive than default 10%)
//!         .min_throughput(50)  // Require minimum 50 executions before considering circuit open
//!         .sampling_duration(Duration::from_secs(60)) // Evaluate failures over 60-second window
//!         .break_duration(Duration::from_secs(30))    // Stay open for 30 seconds before testing
//!         // You can provide your own breaker ID logic if needed. The default is a single global
//!         // circuit. By providing distinct IDs, you can have separate circuits for different inputs.
//!         .breaker_id(|input| BreakerId::from(detect_breaker_id(input)))
//!         // State change callbacks for monitoring and alerting
//!         .on_opened(|output, _args| {
//!             println!("circuit breaker OPENED due to failure: {:?}", output);
//!             // In real code, this would trigger alerts, metrics, logging, etc.
//!         })
//!         .on_closed(|output, _args| {
//!             println!("circuit breaker CLOSED, service recovered: {:?}", output);
//!             // In real code, this would log recovery, update dashboards, etc.
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
mod layer;
mod service;
#[doc(inline)]
pub use args::{OnClosedArgs, OnOpenedArgs, OnProbingArgs, RecoveryArgs, RejectedInputArgs};
pub(super) use callbacks::*;
#[doc(inline)]
pub use layer::BreakerLayer;
#[doc(inline)]
pub use service::Breaker;

mod execution_result;
pub(super) use execution_result::ExecutionResult;

mod health;
pub(super) use health::*;

mod constants;
mod engine;

#[cfg(any(feature = "metrics", test))]
mod telemetry;
pub(super) use engine::*;

mod breaker_id;
pub use breaker_id::BreakerId;

mod half_open_mode;
pub use half_open_mode::HalfOpenMode;
