// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry implementation and recording.

use std::time::Duration;

use arrayvec::ArrayVec;
use opentelemetry::{
    KeyValue,
    logs::Severity,
    metrics::{Counter, Gauge, Histogram, Meter},
};
use thread_aware::Arc;
use tick::Clock;

use crate::{
    cache::CacheName,
    telemetry::{
        CacheActivity, CacheOperation, CacheTelemetry, attributes,
        metrics::{create_cache_size_gauge, create_event_counter, create_operation_duration_histogram},
    },
};

/// Maximum attributes per event: `cache_name`, event, event, `duration_ns`, reason = 5
const MAX_ATTRIBUTES: usize = 5;

type Attributes = ArrayVec<KeyValue, MAX_ATTRIBUTES>;

#[derive(Clone, Debug)]
pub(crate) struct CacheTelemetryInner {
    clock: Clock,
    logging_enabled: bool,
    event_counter: Option<Counter<u64>>,
    operation_duration: Option<Histogram<f64>>,
    cache_size: Option<Gauge<u64>>,
}

impl CacheTelemetry {
    /// Creates a new cache telemetry collector.
    ///
    /// # Arguments
    ///
    /// * `telemetry` - The oxidizer telemetry instance to use
    /// * `clock` - The clock to use for timing events
    #[must_use]
    pub fn new(logging_enabled: bool, meter: Option<&Meter>, clock: Clock) -> Self {
        Self {
            inner: Arc::from_unaware(CacheTelemetryInner {
                logging_enabled,
                clock,
                event_counter: meter.map(create_event_counter),
                operation_duration: meter.map(create_operation_duration_histogram),
                cache_size: meter.map(create_cache_size_gauge),
            }),
        }
    }

    /// Returns a reference to the clock used for timing events.
    #[inline]
    #[must_use]
    pub fn clock(&self) -> &Clock {
        &self.inner.clock
    }

    /// Records a cache operation.
    ///
    /// # Arguments
    ///
    /// * `cache_name` - Static string identifying the cache instance
    /// * `operation` - The type of cache operation
    /// * `activity` - The operation activity
    /// * `duration` - Optional operation duration
    #[inline]
    pub(crate) fn record(&self, cache_name: CacheName, operation: CacheOperation, activity: CacheActivity, duration: Option<Duration>) {
        let mut attrs = Attributes::new();

        attrs.push(KeyValue::new(attributes::CACHE_NAME, cache_name));
        attrs.push(KeyValue::new(attributes::CACHE_OPERATION_NAME, operation.as_str()));
        attrs.push(KeyValue::new(attributes::CACHE_ACTIVITY_NAME, activity.as_str()));

        if let Some(c) = &self.inner.event_counter {
            c.add(1, &attrs);
        }

        // Record duration histogram if duration is provided
        if let (Some(d), Some(h)) = (duration, &self.inner.operation_duration) {
            h.record(d.as_secs_f64(), &attrs);
        }

        if self.inner.logging_enabled {
            Self::emit(cache_name, operation, activity, duration);
        }
    }

    /// Records the current cache size.
    ///
    /// # Arguments
    ///
    /// * `cache_name` - Static string identifying the cache instance
    /// * `size` - The current number of entries in the cache
    #[inline]
    pub(crate) fn record_size(&self, cache_name: CacheName, size: u64) {
        let attrs = [KeyValue::new(attributes::CACHE_NAME, cache_name)];
        if let Some(g) = &self.inner.cache_size {
            g.record(size, &attrs);
        }
    }

    fn emit(cache_name: CacheName, operation: CacheOperation, event: CacheActivity, duration: Option<Duration>) {
        let op = operation.as_str();
        let ev = event.as_str();
        let duration_ns = duration.map(|d| d.as_nanos());

        // Tracing level must be constant, so we use a macro to select the appropriate level.
        // Field names must match constants in attributes.rs - see attribute_names_match_tracing_fields test.
        macro_rules! emit_event {
            ($level:ident) => {
                tracing::$level!(
                    cache.name = cache_name,
                    cache.operation = op,
                    cache.activity = ev,
                    cache.duration_ns = ?duration_ns,
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

    use opentelemetry::metrics::MeterProvider;

    use crate::telemetry::testing::{LogCapture, MetricTester};

    #[test]
    fn metrics_record_emits_correct_attributes() {
        let tester = MetricTester::new();
        let meter = tester.meter_provider().meter("cache");
        let telemetry = CacheTelemetry::new(false, Some(&meter), Clock::new_frozen());

        telemetry.record("my_cache", CacheOperation::Get, CacheActivity::Hit, Some(Duration::from_millis(5)));

        tester.assert_attributes_contain(&[
            opentelemetry::KeyValue::new(attributes::CACHE_NAME, "my_cache"),
            opentelemetry::KeyValue::new(attributes::CACHE_OPERATION_NAME, CacheOperation::Get.as_str()),
            opentelemetry::KeyValue::new(attributes::CACHE_ACTIVITY_NAME, CacheActivity::Hit.as_str()),
        ]);
    }

    #[test]
    fn metrics_record_size_emits_cache_name() {
        let tester = MetricTester::new();
        let meter = tester.meter_provider().meter("cache");
        let telemetry = CacheTelemetry::new(false, Some(&meter), Clock::new_frozen());

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
            Some(Duration::from_nanos(12345)),
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
    }

    #[test]
    fn logs_emit_at_correct_severity_levels() {
        // Error level - should always be captured
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        CacheTelemetry::emit("cache", CacheOperation::Get, CacheActivity::Error, None);
        capture.assert_contains("ERROR");

        // Info level
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        CacheTelemetry::emit("cache", CacheOperation::Get, CacheActivity::Expired, None);
        capture.assert_contains("INFO");

        // Debug level
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());
        CacheTelemetry::emit("cache", CacheOperation::Get, CacheActivity::Hit, None);
        capture.assert_contains("DEBUG");
    }

    #[test]
    fn telemetry_disabled_emits_nothing() {
        // No meter, no logs
        let telemetry = CacheTelemetry::new(false, None, Clock::new_frozen());

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        // This should not panic and should not emit logs
        telemetry.record("cache", CacheOperation::Get, CacheActivity::Hit, Some(Duration::from_secs(1)));

        assert!(capture.output().is_empty());
    }
}
