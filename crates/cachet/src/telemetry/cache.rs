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
    pub(crate) logging_enabled: bool,
}

#[cfg(any(feature = "logs", test))]
impl CacheTelemetryInner {
    #[inline]
    fn debug(&self, cache_name: CacheName, event: &'static str, duration: Duration) {
        if self.logging_enabled {
            tracing::debug!(
                cache.name = cache_name,
                cache.event = event,
                cache.duration_ns = duration.as_nanos()
            );
        }
    }

    #[inline]
    fn info(&self, cache_name: CacheName, event: &'static str, duration: Duration) {
        if self.logging_enabled {
            tracing::info!(
                cache.name = cache_name,
                cache.event = event,
                cache.duration_ns = duration.as_nanos()
            );
        }
    }

    #[inline]
    fn error(&self, cache_name: CacheName, event: &'static str, duration: Duration) {
        if self.logging_enabled {
            tracing::error!(
                cache.name = cache_name,
                cache.event = event,
                cache.duration_ns = duration.as_nanos()
            );
        }
    }
}

/// Internal state for cache telemetry when no features are enabled (no-op).
#[cfg(not(any(feature = "logs", test)))]
#[derive(Clone, Debug, Default)]
pub(crate) struct CacheTelemetryInner;

#[cfg(not(any(feature = "logs", test)))]
#[expect(clippy::unused_self, reason = "Methods must match the logs-enabled impl signature")]
impl CacheTelemetryInner {
    #[inline]
    fn debug(&self, _: CacheName, _: &'static str, _: Duration) {}

    #[inline]
    fn info(&self, _: CacheName, _: &'static str, _: Duration) {}

    #[inline]
    fn error(&self, _: CacheName, _: &'static str, _: Duration) {}
}

/// Cache telemetry provider.
///
/// This type is created internally by the cache builder and handles
/// recording cache operations as structured tracing events.
#[derive(Clone, Debug)]
pub struct CacheTelemetry {
    #[cfg(any(feature = "logs", test))]
    pub(crate) inner: Arc<CacheTelemetryInner, PerCore>,
    #[cfg(not(any(feature = "logs", test)))]
    pub(crate) inner: CacheTelemetryInner,
}

impl CacheTelemetry {
    /// Creates a new `CacheTelemetry` with logging disabled.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::with_options(false)
    }

    /// Creates a new `CacheTelemetry` with logging enabled.
    #[cfg(any(feature = "logs", test))]
    #[must_use]
    pub(crate) fn with_logging() -> Self {
        Self::with_options(true)
    }

    #[cfg(any(feature = "logs", test))]
    fn with_options(logging_enabled: bool) -> Self {
        Self {
            inner: Arc::from_unaware(CacheTelemetryInner { logging_enabled }),
        }
    }

    #[cfg(not(any(feature = "logs", test)))]
    fn with_options(_logging_enabled: bool) -> Self {
        Self {
            inner: CacheTelemetryInner,
        }
    }

    // -- Get --

    /// Records a cache hit (key found and not expired).
    #[inline]
    pub(crate) fn cache_hit(&self, cache_name: CacheName, duration: Duration) {
        self.inner.debug(cache_name, attributes::EVENT_HIT, duration);
    }

    /// Records a cache miss (key not found).
    #[inline]
    pub(crate) fn cache_miss(&self, cache_name: CacheName, duration: Duration) {
        self.inner.debug(cache_name, attributes::EVENT_MISS, duration);
    }

    /// Records a cache entry that was found but expired.
    #[inline]
    pub(crate) fn cache_expired(&self, cache_name: CacheName, duration: Duration) {
        self.inner.info(cache_name, attributes::EVENT_EXPIRED, duration);
    }

    /// Records an error during a get operation.
    #[inline]
    pub(crate) fn get_error(&self, cache_name: CacheName, duration: Duration) {
        self.inner.error(cache_name, attributes::EVENT_GET_ERROR, duration);
    }

    /// Records a fallback tier lookup.
    #[inline]
    pub(crate) fn cache_fallback(&self, cache_name: CacheName, duration: Duration) {
        self.inner.info(cache_name, attributes::EVENT_FALLBACK, duration);
    }

    // -- Refresh --

    /// Records a successful background refresh from fallback.
    #[inline]
    pub(crate) fn refresh_hit(&self, cache_name: CacheName, duration: Duration) {
        self.inner.debug(cache_name, attributes::EVENT_REFRESH_HIT, duration);
    }

    /// Records a background refresh miss (fallback had no data or returned error).
    #[inline]
    pub(crate) fn refresh_miss(&self, cache_name: CacheName, duration: Duration) {
        self.inner.info(cache_name, attributes::EVENT_REFRESH_MISS, duration);
    }

    // -- Insert --

    /// Records a successful cache insert.
    #[inline]
    pub(crate) fn cache_inserted(&self, cache_name: CacheName, duration: Duration) {
        self.inner.info(cache_name, attributes::EVENT_INSERTED, duration);
    }

    /// Records a cache insert that was rejected by the insert policy.
    #[inline]
    pub(crate) fn insert_rejected(&self, cache_name: CacheName, duration: Duration) {
        self.inner.info(cache_name, attributes::EVENT_INSERT_REJECTED, duration);
    }

    /// Records an error during an insert operation.
    #[inline]
    pub(crate) fn insert_error(&self, cache_name: CacheName, duration: Duration) {
        self.inner.error(cache_name, attributes::EVENT_INSERT_ERROR, duration);
    }

    // -- Invalidate --

    /// Records a successful cache invalidation.
    #[inline]
    pub(crate) fn cache_invalidated(&self, cache_name: CacheName, duration: Duration) {
        self.inner.info(cache_name, attributes::EVENT_INVALIDATED, duration);
    }

    /// Records an error during an invalidate operation.
    #[inline]
    pub(crate) fn invalidate_error(&self, cache_name: CacheName, duration: Duration) {
        self.inner.error(cache_name, attributes::EVENT_INVALIDATE_ERROR, duration);
    }

    // -- Clear --

    /// Records a successful cache clear.
    #[inline]
    pub(crate) fn cache_cleared(&self, cache_name: CacheName, duration: Duration) {
        self.inner.debug(cache_name, attributes::EVENT_CLEARED, duration);
    }

    /// Records an error during a clear operation.
    #[inline]
    pub(crate) fn clear_error(&self, cache_name: CacheName, duration: Duration) {
        self.inner.error(cache_name, attributes::EVENT_CLEAR_ERROR, duration);
    }
}

#[cfg(test)]
mod tests {
    use testing_aids::LogCapture;

    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn logs_emit_contains_all_fields_and_values() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let telemetry = CacheTelemetry::with_logging();
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
        let telemetry = CacheTelemetry::with_logging();

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
        let telemetry = CacheTelemetry::new();

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        telemetry.cache_hit("cache", Duration::from_secs(1));

        assert!(capture.output().is_empty());
    }
}
