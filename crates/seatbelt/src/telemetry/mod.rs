// Copyright (c) Microsoft Corporation.

//! # Service Middleware Telemetry
//!
//! This module defines well‑known attribute / label keys used when emitting
//! telemetry (metrics, traces, logs) from middleware components such as retry,
//! timeout, bulkhead, circuit breaker, and fallback pipelines.
//!
//! The constants are stable keys you can attach to:
//!
//! - metrics (e.g. counters, histograms)
//! - tracing spans / events
//! - structured log records
//!
//! # Conventions
//!
//! Names follow the [OpenTelemetry naming guidelines](https://opentelemetry.io/docs/specs/semconv/general/naming/#general-naming-considerations).
//!
//! - Keys should be dot-separated (e.g., `pipeline.name`, `strategy.name`)
//! - Values should be concise and short, preferably using `snake_case`

#[cfg(any(feature = "metrics", test))]
pub(crate) mod metrics;

/// Key used to annotate the name of a resilience pipeline.
///
/// Values reported under this dimension should be short and concise, preferably in `snake_case`.
/// Examples: `user_auth`, `data_processing`, `payment_flow`.
pub const PIPELINE_NAME: &str = "resilience.pipeline.name";

/// Key used to annotate the name of a resilience strategy.
///
/// Values reported under this dimension should be short and concise, preferably in `snake_case`.
/// Examples: `retry`, `circuit_breaker`, `timeout`, `bulkhead`.
pub const STRATEGY_NAME: &str = "resilience.strategy.name";

/// Key used to annotate the specific resilience event being emitted.
///
/// Values reported under this dimension should be short and concise, preferably in `snake_case`.
/// Examples: `retry`, `timeout`, `circuit_opened`.
pub const EVENT_NAME: &str = "resilience.event.name";

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_name_is_expected() {
        assert_eq!(PIPELINE_NAME, "resilience.pipeline.name");
    }

    #[test]
    fn test_strategy_name_is_expected() {
        assert_eq!(STRATEGY_NAME, "resilience.strategy.name");
    }

    #[test]
    fn test_event_name_is_expected() {
        assert_eq!(EVENT_NAME, "resilience.event.name");
    }
}
