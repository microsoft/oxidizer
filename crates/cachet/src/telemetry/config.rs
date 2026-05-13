// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry configuration for cache operations.

#[cfg(any(feature = "logs", test))]
use thread_aware::Arc;

use crate::telemetry::CacheTelemetry;
#[cfg(any(feature = "logs", test))]
use crate::telemetry::cache::CacheTelemetryInner;

/// Configuration for cache telemetry.
///
/// This is an internal builder used by [`CacheBuilder`](crate::CacheBuilder) to
/// collect telemetry settings. Users configure telemetry through the cache
/// builder's [`enable_logs()`](crate::CacheBuilder::enable_logs) method.
#[derive(Clone, Debug, Default)]
pub(crate) struct TelemetryConfig {
    #[cfg(any(feature = "logs", test))]
    pub(crate) logs_enabled: bool,
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

    /// Builds the telemetry collector from this configuration.
    #[must_use]
    pub(crate) fn build(self) -> CacheTelemetry {
        #[cfg(any(feature = "logs", test))]
        {
            CacheTelemetry {
                inner: Arc::from_unaware(CacheTelemetryInner {
                    logging_enabled: self.logs_enabled,
                }),
            }
        }

        #[cfg(not(any(feature = "logs", test)))]
        {
            _ = self;
            #[expect(clippy::default_trait_access, reason = "CacheTelemetryInner is not in scope without feature flags")]
            CacheTelemetry { inner: Default::default() }
        }
    }
}
