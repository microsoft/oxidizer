// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry types and recording.

use std::time::Duration;

#[cfg(any(feature = "logs", test))]
use thread_aware::{Arc, PerCore};

use crate::cache::CacheName;

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

#[derive(Debug, Clone, Copy)]
pub(crate) enum CacheOperation {
    Get,
    Insert,
    Invalidate,
    Clear,
}

#[cfg(any(feature = "logs", test))]
impl CacheOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "cache.get",
            Self::Insert => "cache.insert",
            Self::Invalidate => "cache.invalidate",
            Self::Clear => "cache.clear",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CacheActivity {
    Hit,
    Error,
    Expired,
    Fallback,
    Inserted,
    Invalidated,
    Miss,
    Ok,
    RefreshHit,
    RefreshMiss,
    Rejected,
}

#[cfg(any(feature = "logs", test))]
impl CacheActivity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "cache.hit",
            Self::Error => "cache.error",
            Self::Expired => "cache.expired",
            Self::Fallback => "cache.fallback",
            Self::Inserted => "cache.inserted",
            Self::Invalidated => "cache.invalidated",
            Self::Miss => "cache.miss",
            Self::Ok => "cache.ok",
            Self::RefreshHit => "cache.refresh_hit",
            Self::RefreshMiss => "cache.refresh_miss",
            Self::Rejected => "cache.rejected",
        }
    }
}

impl CacheTelemetry {
    /// Records a cache operation with the given name, type, activity, and duration.
    #[cfg_attr(
        not(any(feature = "logs", test)),
        expect(unused_variables, reason = "No-op when both logs are disabled")
    )]
    #[cfg_attr(
        not(any(feature = "logs", test)),
        expect(clippy::unused_self, reason = "self is used under feature flags")
    )]
    #[inline]
    fn record(&self, cache_name: CacheName, operation: CacheOperation, activity: CacheActivity, duration: Duration) {
        #[cfg(any(feature = "logs", test))]
        if self.inner.logging_enabled {
            Self::emit(cache_name, operation, activity, duration);
        }
    }

    // -- Get operation helpers --

    /// Records a cache hit (key found and not expired).
    #[inline]
    pub(crate) fn cache_hit(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Get, CacheActivity::Hit, duration);
    }

    /// Records a cache miss (key not found).
    #[inline]
    pub(crate) fn cache_miss(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Get, CacheActivity::Miss, duration);
    }

    /// Records a cache entry that was found but expired.
    #[inline]
    pub(crate) fn cache_expired(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Get, CacheActivity::Expired, duration);
    }

    /// Records an error during a get operation.
    #[inline]
    pub(crate) fn get_error(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Get, CacheActivity::Error, duration);
    }

    /// Records a fallback tier lookup.
    #[inline]
    pub(crate) fn cache_fallback(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Get, CacheActivity::Fallback, duration);
    }

    /// Records a successful background refresh from fallback.
    #[inline]
    pub(crate) fn refresh_hit(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Get, CacheActivity::RefreshHit, duration);
    }

    /// Records a background refresh miss (fallback had no data or errored).
    #[inline]
    pub(crate) fn refresh_miss(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Get, CacheActivity::RefreshMiss, duration);
    }

    // -- Insert operation helpers --

    /// Records a successful cache insert.
    #[inline]
    pub(crate) fn cache_inserted(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Insert, CacheActivity::Inserted, duration);
    }

    /// Records a cache insert that was rejected by the insert policy.
    #[inline]
    pub(crate) fn insert_rejected(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Insert, CacheActivity::Rejected, duration);
    }

    /// Records an error during an insert operation.
    #[inline]
    pub(crate) fn insert_error(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Insert, CacheActivity::Error, duration);
    }

    // -- Invalidate operation helpers --

    /// Records a successful cache invalidation.
    #[inline]
    pub(crate) fn cache_invalidated(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Invalidate, CacheActivity::Invalidated, duration);
    }

    /// Records an error during an invalidate operation.
    #[inline]
    pub(crate) fn invalidate_error(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Invalidate, CacheActivity::Error, duration);
    }

    // -- Clear operation helpers --

    /// Records a successful cache clear.
    #[inline]
    pub(crate) fn cache_cleared(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Clear, CacheActivity::Ok, duration);
    }

    /// Records an error during a clear operation.
    #[inline]
    pub(crate) fn clear_error(&self, cache_name: CacheName, duration: Duration) {
        self.record(cache_name, CacheOperation::Clear, CacheActivity::Error, duration);
    }

    #[cfg(any(feature = "logs", test))]
    fn emit(cache_name: CacheName, operation: CacheOperation, event: CacheActivity, duration: Duration) {
        let op = operation.as_str();
        let ev = event.as_str();
        let duration_ns = duration.as_nanos();

        // Tracing level must be a constant, so we match on each level separately.
        // The default tracing target is the module path (cachet::telemetry::cache),
        // which consumers can filter with `Targets::new().with_target("cachet", ...)`.
        // Field names must match constants in attributes.rs — see logs_emit_contains_all_fields_and_values test.
        macro_rules! emit_event {
            ($level:ident) => {
                tracing::$level!(
                    cache.name = cache_name,
                    cache.operation = op,
                    cache.activity = ev,
                    cache.duration_ns = duration_ns,
                    "cache.event"
                )
            };
        }

        match event {
            CacheActivity::Error => emit_event!(error),
            CacheActivity::Expired
            | CacheActivity::RefreshMiss
            | CacheActivity::Inserted
            | CacheActivity::Invalidated
            | CacheActivity::Fallback
            | CacheActivity::Rejected => emit_event!(info),
            CacheActivity::Hit | CacheActivity::Miss | CacheActivity::RefreshHit | CacheActivity::Ok => {
                emit_event!(debug);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use testing_aids::LogCapture;

    use super::*;
    use crate::telemetry::{TelemetryConfig, attributes};

    #[test]
    fn cache_operation_as_str() {
        assert_eq!(CacheOperation::Get.as_str(), "cache.get");
        assert_eq!(CacheOperation::Insert.as_str(), "cache.insert");
        assert_eq!(CacheOperation::Invalidate.as_str(), "cache.invalidate");
        assert_eq!(CacheOperation::Clear.as_str(), "cache.clear");
    }

    #[test]
    fn cache_activity_as_str() {
        assert_eq!(CacheActivity::Hit.as_str(), "cache.hit");
        assert_eq!(CacheActivity::Expired.as_str(), "cache.expired");
        assert_eq!(CacheActivity::Miss.as_str(), "cache.miss");
        assert_eq!(CacheActivity::RefreshHit.as_str(), "cache.refresh_hit");
        assert_eq!(CacheActivity::RefreshMiss.as_str(), "cache.refresh_miss");
        assert_eq!(CacheActivity::Inserted.as_str(), "cache.inserted");
        assert_eq!(CacheActivity::Invalidated.as_str(), "cache.invalidated");
        assert_eq!(CacheActivity::Ok.as_str(), "cache.ok");
        assert_eq!(CacheActivity::Fallback.as_str(), "cache.fallback");
        assert_eq!(CacheActivity::Error.as_str(), "cache.error");
        assert_eq!(CacheActivity::Rejected.as_str(), "cache.rejected");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn logs_emit_contains_all_fields_and_values() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let telemetry = TelemetryConfig::new().with_logs().build();
        telemetry.record(
            "my_test_cache",
            CacheOperation::Invalidate,
            CacheActivity::Error,
            Duration::from_nanos(12345),
        );

        // Verify field names
        capture.assert_contains(attributes::CACHE_NAME);
        capture.assert_contains(attributes::CACHE_OPERATION_NAME);
        capture.assert_contains(attributes::CACHE_ACTIVITY_NAME);
        capture.assert_contains("cache.duration_ns");
        capture.assert_contains("cache.event");

        // Verify values
        capture.assert_contains("my_test_cache");
        capture.assert_contains(CacheOperation::Invalidate.as_str());
        capture.assert_contains(CacheActivity::Error.as_str());
        capture.assert_contains("12345");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn logs_emit_at_correct_severity_levels() {
        // Error level - should always be captured
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        CacheTelemetry::emit("cache", CacheOperation::Get, CacheActivity::Error, Duration::ZERO);
        capture.assert_contains("ERROR");

        // Info level
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        CacheTelemetry::emit("cache", CacheOperation::Get, CacheActivity::Expired, Duration::ZERO);
        capture.assert_contains("INFO");

        // Debug level
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        CacheTelemetry::emit("cache", CacheOperation::Get, CacheActivity::Hit, Duration::ZERO);
        capture.assert_contains("DEBUG");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn telemetry_disabled_emits_nothing() {
        // No meter, no logs - telemetry still gets created but does nothing
        let telemetry = TelemetryConfig::new().build();

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        // This should not panic and should not emit logs
        telemetry.record("cache", CacheOperation::Get, CacheActivity::Hit, Duration::from_secs(1));

        assert!(capture.output().is_empty());
    }
}
