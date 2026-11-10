// Copyright (c) Microsoft Corporation.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! OTLP log and metric destinations.
//!
//! Use these destinations with `observed_otel` or register them
//! directly on OpenTelemetry SDK provider builders:
//!
//! - [`otlp_logs`] sends batched logs.
//! - [`otlp_metrics`] sends periodic metrics.
//!
//! Both builders default to HTTP/protobuf and accept [`OtlpDestinationProtocol`]
//! to select gRPC instead.
//!
//! Metric temporality describes the time range covered by an aggregation:
//! cumulative covers time since its starting point, while delta covers the interval
//! since the preceding collection. The export interval controls how often metrics
//! are sent. See [`Temporality`](opentelemetry_sdk::metrics::Temporality) for per-instrument details.
//!
//! Timeouts, metric intervals, and metric temporality are chosen in this order:
//!
//! 1. An explicit setting on the destination builder.
//! 2. The corresponding OpenTelemetry environment variable.
//! 3. The default: a 10-second export timeout, a 60-second metric interval,
//!    and cumulative metric temporality.
//!
//! Environment variables:
//!
//! - `OTEL_EXPORTER_OTLP_TIMEOUT`: general timeout when no signal-specific timeout is set.
//! - `OTEL_EXPORTER_OTLP_LOGS_TIMEOUT`: log export timeout; overrides the general timeout.
//! - `OTEL_EXPORTER_OTLP_METRICS_TIMEOUT`: metric export timeout; overrides the general timeout.
//! - `OTEL_METRIC_EXPORT_INTERVAL`: interval between metric exports.
//! - `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE`: metric aggregation temporality.
//!
//! Timeout and interval environment values are in milliseconds. A zero builder
//! interval is ignored.
//!
//! Registering either signal with gRPC requires an entered Tokio runtime with
//! I/O and timers enabled. Keep that runtime running through provider shutdown.
//!
//! HTTP export uses Reqwest's blocking client on the processor's or reader's
//! background thread. Reqwest's provider-less Rustls backend requires a process-wide crypto
//! provider before client creation.
//!
//! Call the SDK provider's `force_flush` to export pending telemetry immediately,
//! and call `shutdown` before the application exits. These operations block;
//! when called from async code, use a blocking task so the runtime can continue
//! to drive network I/O.
//!
//! # Example
//!
//! ```
//! use observed_destination_otlp::{OtlpDestinationProtocol, otlp_logs, otlp_metrics};
//!
//! let logs = otlp_logs("http://localhost:4317")
//!     .protocol(OtlpDestinationProtocol::Grpc)
//!     .build();
//!
//! let metrics = otlp_metrics("http://localhost:4318/v1/metrics").build();
//! ```

use std::time::Duration;

pub use logs::{OtlpLogDestination, OtlpLogDestinationBuilder, otlp_logs};
pub use metrics::{OtlpMetricDestination, OtlpMetricDestinationBuilder, otlp_metrics};

/// Transport for an OTLP log or metric destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OtlpDestinationProtocol {
    /// HTTP/protobuf with a blocking client.
    Http,
    /// gRPC through Tonic.
    ///
    /// Registration requires an entered Tokio runtime with I/O and timers
    /// enabled, which must remain running through provider shutdown.
    Grpc,
}

fn with_optional_timeout<B>(builder: B, timeout: Option<Duration>, with_timeout: impl FnOnce(B, Duration) -> B) -> B {
    if let Some(timeout) = timeout {
        with_timeout(builder, timeout)
    } else {
        builder
    }
}

mod http_client;
mod logs;
mod metrics;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::with_optional_timeout;

    #[test]
    fn explicit_timeout_overrides_existing_setting() {
        let existing_timeout = Duration::from_secs(10);

        for timeout in [Duration::ZERO, Duration::from_millis(123)] {
            let configured = with_optional_timeout(existing_timeout, Some(timeout), |_, timeout| timeout);

            assert_eq!(configured, timeout);
        }
    }

    #[test]
    fn unset_timeout_preserves_existing_setting() {
        let existing_timeout = Duration::from_millis(456);

        let configured = with_optional_timeout(existing_timeout, None, |_, _| panic!("An unset timeout must not invoke the setter"));

        assert_eq!(configured, existing_timeout);
    }
}
