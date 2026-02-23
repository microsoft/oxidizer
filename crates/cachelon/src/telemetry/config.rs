// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry configuration for cache operations.

#[cfg(any(feature = "metrics", test))]
use opentelemetry::metrics::{Meter, MeterProvider};

use crate::telemetry::CacheTelemetry;
#[cfg(any(feature = "logs", feature = "metrics", test))]
use crate::telemetry::cache::CacheTelemetryInner;
#[cfg(any(feature = "logs", feature = "metrics", test))]
use thread_aware::Arc;

/// Configuration for cache telemetry.
///
/// Use the builder methods to enable logs and/or metrics, then pass
/// this to your cache builder via `.telemetry()`.
///
/// # Examples
///
/// ```ignore
/// use cachelon::TelemetryConfig;
///
/// // Enable logging only
/// let config = TelemetryConfig::new().with_logs();
///
/// // Enable metrics only
/// let config = TelemetryConfig::new().with_metrics(&meter_provider);
///
/// // Enable both
/// let config = TelemetryConfig::new()
///     .with_logs()
///     .with_metrics(&meter_provider);
/// ```
#[derive(Clone, Debug, Default)]
pub(crate) struct TelemetryConfig {
    #[cfg(any(feature = "logs", test))]
    logs_enabled: bool,
    #[cfg(any(feature = "metrics", test))]
    meter: Option<Meter>,
}

impl TelemetryConfig {
    /// Creates a new telemetry configuration with everything disabled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables structured logging for cache operations.
    ///
    /// When enabled, cache operations will emit structured logs via the `tracing` crate.
    #[cfg(any(feature = "logs", test))]
    #[must_use]
    pub(crate) fn with_logs(self) -> Self {
        Self {
            logs_enabled: true,
            ..self
        }
    }

    /// Enables metrics collection using the provided meter provider.
    ///
    /// When enabled, cache operations will emit metrics via OpenTelemetry.
    #[cfg(any(feature = "metrics", test))]
    #[must_use]
    pub fn with_metrics(mut self, provider: &dyn MeterProvider) -> Self {
        use crate::telemetry::metrics;
        self.meter = Some(metrics::create_meter(provider));
        self
    }

    /// Builds the telemetry collector from this configuration.
    #[must_use]
    pub(crate) fn build(self) -> CacheTelemetry {
        #[cfg(not(any(feature = "logs", feature = "metrics", test)))]
        {
            return CacheTelemetry {};
        }

        #[cfg(any(feature = "logs", feature = "metrics", test))]
        {
            #[cfg(any(feature = "metrics", test))]
            let (event_counter, operation_duration, cache_size) = {
                use crate::telemetry::metrics::{create_cache_size_gauge, create_event_counter, create_operation_duration_histogram};
                (
                    self.meter.as_ref().map(create_event_counter),
                    self.meter.as_ref().map(create_operation_duration_histogram),
                    self.meter.as_ref().map(create_cache_size_gauge),
                )
            };

            CacheTelemetry {
                inner: Arc::from_unaware(CacheTelemetryInner {
                    #[cfg(any(feature = "logs", test))]
                    logging_enabled: self.logs_enabled,
                    #[cfg(any(feature = "metrics", test))]
                    event_counter,
                    #[cfg(any(feature = "metrics", test))]
                    operation_duration,
                    #[cfg(any(feature = "metrics", test))]
                    cache_size,
                }),
            }
        }
    }
}
