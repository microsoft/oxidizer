// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry types and recording.

use std::time::Duration;

#[cfg(any(feature = "logs", test))]
use opentelemetry::logs::Severity;
#[cfg(any(feature = "metrics", test))]
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram},
};

use crate::cache::CacheName;
#[cfg(any(feature = "metrics", test))]
use crate::telemetry::attributes;
#[cfg(any(feature = "logs", feature = "metrics", test))]
use thread_aware::{Arc, PerCore};

/// Internal state for cache telemetry.
#[cfg(any(feature = "logs", feature = "metrics", test))]
#[derive(Clone, Debug)]
pub(crate) struct CacheTelemetryInner {
    #[cfg(any(feature = "logs", test))]
    pub(crate) logging_enabled: bool,
    #[cfg(any(feature = "metrics", test))]
    pub(crate) event_counter: Option<Counter<u64>>,
    #[cfg(any(feature = "metrics", test))]
    pub(crate) operation_duration: Option<Histogram<f64>>,
    #[cfg(any(feature = "metrics", test))]
    pub(crate) cache_size: Option<Gauge<u64>>,
}

/// Cache telemetry provider for OpenTelemetry integration.
///
/// This type is created internally by [`TelemetryConfig::build()`] and handles
/// recording cache operations as structured logs and metrics.
#[derive(Clone, Debug)]
pub struct CacheTelemetry {
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    pub(crate) inner: Arc<CacheTelemetryInner, PerCore>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CacheOperation {
    Get,
    Insert,
    Invalidate,
    Clear,
}

#[cfg(any(feature = "logs", feature = "metrics", test))]
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
    Expired,
    Miss,
    RefreshHit,
    RefreshMiss,
    Inserted,
    Invalidated,
    Ok,
    #[cfg_attr(not(test), expect(dead_code, reason = "activity variant for future use"))]
    Fallback,
    FallbackPromotion,
    Error,
}

#[cfg(any(feature = "logs", feature = "metrics", test))]
impl CacheActivity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "cache.hit",
            Self::Expired => "cache.expired",
            Self::Miss => "cache.miss",
            Self::RefreshHit => "cache.refresh_hit",
            Self::RefreshMiss => "cache.refresh_miss",
            Self::Inserted => "cache.inserted",
            Self::Invalidated => "cache.invalidated",
            Self::Ok => "cache.ok",
            Self::Fallback => "cache.fallback",
            Self::FallbackPromotion => "cache.fallback_promotion",
            Self::Error => "cache.error",
        }
    }

    #[cfg(any(feature = "logs", test))]
    pub fn severity(self) -> Severity {
        match self {
            Self::Hit | Self::Miss | Self::RefreshHit | Self::Ok => Severity::Debug,
            Self::Expired | Self::RefreshMiss | Self::Inserted | Self::Invalidated | Self::Fallback | Self::FallbackPromotion => {
                Severity::Info
            }
            Self::Error => Severity::Error,
        }
    }
}

impl CacheTelemetry {
    /// Records a cache operation with the given name, type, activity, and duration.
    #[cfg_attr(
        not(any(feature = "logs", feature = "metrics", test)),
        expect(unused_variables, reason = "No-op when both logs and metrics are disabled")
    )]
    #[inline]
    pub(crate) fn record(&self, cache_name: CacheName, operation: CacheOperation, activity: CacheActivity, duration: Duration) {
        #[cfg(any(feature = "metrics", test))]
        {
            let attrs = [
                KeyValue::new(attributes::CACHE_NAME, cache_name),
                KeyValue::new(attributes::CACHE_OPERATION_NAME, operation.as_str()),
                KeyValue::new(attributes::CACHE_ACTIVITY_NAME, activity.as_str()),
            ];

            if let Some(c) = &self.inner.event_counter {
                c.add(1, &attrs);
            }

            if let Some(h) = &self.inner.operation_duration {
                h.record(duration.as_secs_f64(), &attrs);
            }
        }

        #[cfg(any(feature = "logs", test))]
        if self.inner.logging_enabled {
            Self::emit(cache_name, operation, activity, duration);
        }
    }

    /// Records the current cache size for the given cache name.
    #[cfg_attr(
        not(any(feature = "metrics", test)),
        expect(unused_variables, reason = "No-op when metrics are disabled")
    )]
    #[inline]
    pub(crate) fn record_size(&self, cache_name: CacheName, size: u64) {
        #[cfg(any(feature = "metrics", test))]
        {
            let attrs = [KeyValue::new(attributes::CACHE_NAME, cache_name)];

            if let Some(g) = &self.inner.cache_size {
                g.record(size, &attrs);
            }
        }
    }

    #[cfg(any(feature = "logs", test))]
    fn emit(cache_name: CacheName, operation: CacheOperation, event: CacheActivity, duration: Duration) {
        let op = operation.as_str();
        let ev = event.as_str();
        let duration_ns = duration.as_nanos();

        // Tracing level must be constant, so we use a macro to select the appropriate level.
        // Field names must match constants in attributes.rs - see attribute_names_match_tracing_fields test.
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

        match event.severity() {
            Severity::Error => emit_event!(error),
            Severity::Info => emit_event!(info),
            Severity::Debug => emit_event!(debug),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::TelemetryConfig;
    use testing_aids::{LogCapture, MetricTester};

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
        assert_eq!(CacheActivity::FallbackPromotion.as_str(), "cache.fallback_promotion");
        assert_eq!(CacheActivity::Error.as_str(), "cache.error");
    }

    #[test]
    fn cache_activity_severity_debug() {
        assert_eq!(CacheActivity::Hit.severity(), Severity::Debug);
        assert_eq!(CacheActivity::Miss.severity(), Severity::Debug);
        assert_eq!(CacheActivity::RefreshHit.severity(), Severity::Debug);
        assert_eq!(CacheActivity::Ok.severity(), Severity::Debug);
    }

    #[test]
    fn cache_activity_severity_info() {
        assert_eq!(CacheActivity::Expired.severity(), Severity::Info);
        assert_eq!(CacheActivity::RefreshMiss.severity(), Severity::Info);
        assert_eq!(CacheActivity::Inserted.severity(), Severity::Info);
        assert_eq!(CacheActivity::Invalidated.severity(), Severity::Info);
        assert_eq!(CacheActivity::Fallback.severity(), Severity::Info);
        assert_eq!(CacheActivity::FallbackPromotion.severity(), Severity::Info);
    }

    #[test]
    fn cache_activity_severity_error() {
        assert_eq!(CacheActivity::Error.severity(), Severity::Error);
    }

    #[test]
    fn metrics_record_emits_correct_attributes() {
        let tester = MetricTester::new();
        let telemetry = TelemetryConfig::new().with_metrics(tester.meter_provider()).build();

        telemetry.record("my_cache", CacheOperation::Get, CacheActivity::Hit, Duration::from_millis(5));

        tester.assert_attributes_contain(&[
            opentelemetry::KeyValue::new(attributes::CACHE_NAME, "my_cache"),
            opentelemetry::KeyValue::new(attributes::CACHE_OPERATION_NAME, CacheOperation::Get.as_str()),
            opentelemetry::KeyValue::new(attributes::CACHE_ACTIVITY_NAME, CacheActivity::Hit.as_str()),
        ]);
    }

    #[test]
    fn metrics_record_size_emits_cache_name() {
        let tester = MetricTester::new();
        let telemetry = TelemetryConfig::new().with_metrics(tester.meter_provider()).build();

        telemetry.record_size("size_test_cache", 42);

        tester.assert_attributes_contain(&[opentelemetry::KeyValue::new(attributes::CACHE_NAME, "size_test_cache")]);
    }

    #[test]
    fn logs_emit_contains_all_fields_and_values() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        CacheTelemetry::emit(
            "my_test_cache",
            CacheOperation::Invalidate,
            CacheActivity::Error,
            Duration::from_nanos(12345),
        );

        // Verify field names
        capture.assert_contains(attributes::CACHE_NAME);
        capture.assert_contains(attributes::CACHE_OPERATION_NAME);
        capture.assert_contains(attributes::CACHE_ACTIVITY_NAME);
        capture.assert_contains(attributes::CACHE_DURATION_NAME);
        capture.assert_contains(attributes::CACHE_EVENT_NAME);

        // Verify values
        capture.assert_contains("my_test_cache");
        capture.assert_contains(CacheOperation::Invalidate.as_str());
        capture.assert_contains(CacheActivity::Error.as_str());
        capture.assert_contains("12345");
    }

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
