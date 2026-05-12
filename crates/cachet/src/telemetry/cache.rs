// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry types and recording.

use std::time::Duration;

#[cfg(any(feature = "logs", test))]
use thread_aware::{Arc, PerCore};

use crate::cache::CacheName;
use crate::telemetry::attributes;

/// Internal state for cache telemetry when features are enabled.
#[cfg(any(feature = "logs", test))]
#[derive(Clone, Debug)]
pub(crate) struct CacheTelemetryInner {
    #[cfg(any(feature = "logs", test))]
    pub(crate) logging_enabled: bool,
}

/// Internal state for cache telemetry when no features are enabled (no-op).
#[cfg(not(any(feature = "logs", test)))]
#[derive(Clone, Debug, Default)]
pub(crate) struct CacheTelemetryInner;

/// Cache telemetry provider for OpenTelemetry integration.
///
/// This type is created internally by [`TelemetryConfig::build()`] and handles
/// recording cache operations as structured logs.
#[derive(Clone, Debug)]
pub struct CacheTelemetry {
    #[cfg(any(feature = "logs", test))]
    pub(crate) inner: Arc<CacheTelemetryInner, PerCore>,
    #[cfg(not(any(feature = "logs", test)))]
    #[expect(dead_code, reason = "No-op telemetry when features are disabled")]
    pub(crate) inner: CacheTelemetryInner,
}

impl CacheTelemetry {
    // -- Emit helpers (encapsulate the enabled check + cfg gate) --

    #[allow(unused_variables)]
    #[inline]
    fn debug(&self, cache_name: CacheName, event: &'static str, duration: Duration) {
        #[cfg(any(feature = "logs", test))]
        if self.inner.logging_enabled {
            tracing::debug!(
                cache.name = cache_name,
                cache.event = event,
                cache.duration_ns = duration.as_nanos()
            );
        }
    }

    #[allow(unused_variables)]
    #[inline]
    fn info(&self, cache_name: CacheName, event: &'static str, duration: Duration) {
        #[cfg(any(feature = "logs", test))]
        if self.inner.logging_enabled {
            tracing::info!(
                cache.name = cache_name,
                cache.event = event,
                cache.duration_ns = duration.as_nanos()
            );
        }
    }

    #[allow(unused_variables)]
    #[inline]
    fn error(&self, cache_name: CacheName, event: &'static str, duration: Duration) {
        #[cfg(any(feature = "logs", test))]
        if self.inner.logging_enabled {
            tracing::error!(
                cache.name = cache_name,
                cache.event = event,
                cache.duration_ns = duration.as_nanos()
            );
        }
    }

    // -- Get --

    /// Records a cache hit (key found and not expired).
    #[inline]
    pub(crate) fn cache_hit(&self, cache_name: CacheName, duration: Duration) {
        self.debug(cache_name, attributes::EVENT_HIT, duration);
    }

    /// Records a cache miss (key not found).
    #[inline]
    pub(crate) fn cache_miss(&self, cache_name: CacheName, duration: Duration) {
        self.debug(cache_name, attributes::EVENT_MISS, duration);
    }

    /// Records a cache entry that was found but expired.
    #[inline]
    pub(crate) fn cache_expired(&self, cache_name: CacheName, duration: Duration) {
        self.info(cache_name, attributes::EVENT_EXPIRED, duration);
    }

    /// Records an error during a get operation.
    #[inline]
    pub(crate) fn get_error(&self, cache_name: CacheName, duration: Duration) {
        self.error(cache_name, attributes::EVENT_GET_ERROR, duration);
    }

    /// Records a fallback tier lookup.
    #[inline]
    pub(crate) fn cache_fallback(&self, cache_name: CacheName, duration: Duration) {
        self.info(cache_name, attributes::EVENT_FALLBACK, duration);
    }

    // -- Refresh --

    /// Records a successful background refresh from fallback.
    #[inline]
    pub(crate) fn refresh_hit(&self, cache_name: CacheName, duration: Duration) {
        self.debug(cache_name, attributes::EVENT_REFRESH_HIT, duration);
    }

    /// Records a background refresh miss (fallback had no data or errored).
    #[inline]
    pub(crate) fn refresh_miss(&self, cache_name: CacheName, duration: Duration) {
        self.info(cache_name, attributes::EVENT_REFRESH_MISS, duration);
    }

    // -- Insert --

    /// Records a successful cache insert.
    #[inline]
    pub(crate) fn cache_inserted(&self, cache_name: CacheName, duration: Duration) {
        self.info(cache_name, attributes::EVENT_INSERTED, duration);
    }

    /// Records a cache insert that was rejected by the insert policy.
    #[inline]
    pub(crate) fn insert_rejected(&self, cache_name: CacheName, duration: Duration) {
        self.info(cache_name, attributes::EVENT_INSERT_REJECTED, duration);
    }

    /// Records an error during an insert operation.
    #[inline]
    pub(crate) fn insert_error(&self, cache_name: CacheName, duration: Duration) {
        self.error(cache_name, attributes::EVENT_INSERT_ERROR, duration);
    }

    // -- Invalidate --

    /// Records a successful cache invalidation.
    #[inline]
    pub(crate) fn cache_invalidated(&self, cache_name: CacheName, duration: Duration) {
        self.info(cache_name, attributes::EVENT_INVALIDATED, duration);
    }

    /// Records an error during an invalidate operation.
    #[inline]
    pub(crate) fn invalidate_error(&self, cache_name: CacheName, duration: Duration) {
        self.error(cache_name, attributes::EVENT_INVALIDATE_ERROR, duration);
    }

    // -- Clear --

    /// Records a successful cache clear.
    #[inline]
    pub(crate) fn cache_cleared(&self, cache_name: CacheName, duration: Duration) {
        self.debug(cache_name, attributes::EVENT_CLEARED, duration);
    }

    /// Records an error during a clear operation.
    #[inline]
    pub(crate) fn clear_error(&self, cache_name: CacheName, duration: Duration) {
        self.error(cache_name, attributes::EVENT_CLEAR_ERROR, duration);
    }
}

#[cfg(test)]
mod tests {
    use testing_aids::LogCapture;

    use super::*;
    use crate::telemetry::TelemetryConfig;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn logs_emit_contains_all_fields_and_values() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let telemetry = TelemetryConfig::new().with_logs().build();
        telemetry.invalidate_error("my_test_cache", Duration::from_nanos(12345));

        // Verify field names match public constants
        capture.assert_contains(attributes::FIELD_NAME);
        capture.assert_contains(attributes::FIELD_EVENT);
        capture.assert_contains(attributes::FIELD_DURATION_NS);

        // Verify values
        capture.assert_contains("my_test_cache");
        capture.assert_contains(attributes::EVENT_INVALIDATE_ERROR);
        capture.assert_contains("12345");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn logs_emit_at_correct_severity_levels() {
        let telemetry = TelemetryConfig::new().with_logs().build();

        // Error level
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        telemetry.get_error("cache", Duration::ZERO);
        capture.assert_contains("ERROR");

        // Info level
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        telemetry.cache_expired("cache", Duration::ZERO);
        capture.assert_contains("INFO");

        // Debug level
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        telemetry.cache_hit("cache", Duration::ZERO);
        capture.assert_contains("DEBUG");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn telemetry_disabled_emits_nothing() {
        let telemetry = TelemetryConfig::new().build();

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        telemetry.cache_hit("cache", Duration::from_secs(1));

        assert!(capture.output().is_empty());
    }
}
