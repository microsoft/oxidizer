// Copyright (c) Microsoft Corporation.

//! Cache telemetry implementation and recording.

use std::time::Duration;

use arrayvec::ArrayVec;
use opentelemetry::{
    KeyValue,
    logs::{LogRecord, Logger, LoggerProvider},
    metrics::{Counter, Gauge, Histogram, MeterProvider},
};
use opentelemetry_sdk::{logs::SdkLoggerProvider, metrics::SdkMeterProvider};
use thread_aware::Arc;
use tick::Clock;

use crate::{
    cache::CacheName,
    telemetry::{CacheEvent, CacheOperation, CacheTelemetry},
};

const METER_NAME: &str = "cache";
const LOGGER_NAME: &str = "cache";

/// Maximum attributes per event: `cachelon_name`, event, event, `duration_ns`, reason = 5
const MAX_ATTRIBUTES: usize = 5;

type Attributes = ArrayVec<KeyValue, MAX_ATTRIBUTES>;

#[derive(Clone, Debug)]
pub(crate) struct CacheTelemetryInner {
    logger_provider: SdkLoggerProvider,
    clock: Clock,
    operation_counter: Counter<u64>,
    operation_duration: Histogram<f64>,
    cachelon_size: Gauge<u64>,
}

impl CacheTelemetry {
    /// Creates a new cache telemetry collector.
    ///
    /// # Arguments
    ///
    /// * `telemetry` - The oxidizer telemetry instance to use
    /// * `clock` - The clock to use for timing events
    #[must_use]
    pub fn new(logger_provider: SdkLoggerProvider, meter_provider: &SdkMeterProvider, clock: Clock) -> Self {
        let meter = meter_provider.meter(METER_NAME);

        let event_counter = meter
            .u64_counter("cache.event.count")
            .with_description("Cache events")
            .with_unit("{event}")
            .build();

        let operation_duration = meter
            .f64_histogram("cache.operation.duration")
            .with_description("Cache operation duration")
            .with_unit("s")
            .build();

        let cachelon_size = meter
            .u64_gauge("cache.size")
            .with_description("Number of entries in the cache")
            .with_unit("{entry}")
            .build();

        Self {
            inner: Arc::from_unaware(CacheTelemetryInner {
                logger_provider,
                clock,
                operation_counter: event_counter,
                operation_duration,
                cachelon_size,
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
    /// * `cachelon_name` - Static string identifying the cache instance
    /// * `operation` - The type of cache operation
    /// * `event` - The operation event
    /// * `duration` - Optional operation duration
    #[inline]
    pub(crate) fn record(&self, cachelon_name: CacheName, operation: CacheOperation, event: CacheEvent, duration: Option<Duration>) {
        let mut attrs = Attributes::new();

        attrs.push(KeyValue::new("cachelon_name", cachelon_name));
        attrs.push(KeyValue::new("operation", operation.as_str()));
        attrs.push(KeyValue::new("event", event.as_str()));

        self.inner.operation_counter.add(1, &attrs);

        // Record duration histogram if duration is provided
        if let Some(d) = duration {
            self.inner.operation_duration.record(d.as_secs_f64(), &attrs);
        }

        // Log the operation
        let logger = self.inner.logger_provider.logger(LOGGER_NAME);
        let mut record = logger.create_log_record();

        record.set_body("cache.operation".into());
        record.set_severity_text(event.severity().name());
        record.set_severity_number(event.severity());

        record.add_attribute("cachelon_name", cachelon_name);
        record.add_attribute("operation", operation.as_str());
        record.add_attribute("event", event.as_str());

        if let Some(d) = duration {
            #[expect(clippy::cast_possible_truncation, reason = "duration in nanoseconds unlikely to exceed i64::MAX")]
            record.add_attribute("duration_ns", d.as_nanos() as i64);
        }

        logger.emit(record);
    }

    /// Records the current cache size.
    ///
    /// # Arguments
    ///
    /// * `cachelon_name` - Static string identifying the cache instance
    /// * `size` - The current number of entries in the cache
    #[inline]
    pub(crate) fn record_size(&self, cachelon_name: CacheName, size: u64) {
        let attrs = [KeyValue::new("cachelon_name", cachelon_name)];
        self.inner.cachelon_size.record(size, &attrs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use opentelemetry_sdk::metrics::InMemoryMetricExporter;

    fn create_test_providers() -> (SdkLoggerProvider, SdkMeterProvider) {
        let logger_provider = SdkLoggerProvider::builder().build();
        let exporter = InMemoryMetricExporter::default();
        let meter_provider = SdkMeterProvider::builder().with_periodic_exporter(exporter).build();
        (logger_provider, meter_provider)
    }

    #[test]
    fn cachelon_telemetry_new() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();

        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Verify we can access clock
        let _ = telemetry.clock().instant();
    }

    #[test]
    fn cachelon_telemetry_clock() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();

        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Verify clock access works
        let returned_clock = telemetry.clock();
        let _ = returned_clock.instant();
    }

    #[test]
    fn cachelon_telemetry_record_get_hit() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Record a get hit event
        telemetry.record("test_cache", CacheOperation::Get, CacheEvent::Hit, Some(Duration::from_millis(5)));
    }

    #[test]
    fn cachelon_telemetry_record_get_miss() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Record a get miss event
        telemetry.record("test_cache", CacheOperation::Get, CacheEvent::Miss, Some(Duration::from_millis(10)));
    }

    #[test]
    fn cachelon_telemetry_record_insert() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        telemetry.record(
            "test_cache",
            CacheOperation::Insert,
            CacheEvent::Inserted,
            Some(Duration::from_millis(1)),
        );
    }

    #[test]
    fn cachelon_telemetry_record_invalidate() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        telemetry.record(
            "test_cache",
            CacheOperation::Invalidate,
            CacheEvent::Invalidated,
            Some(Duration::from_nanos(500)),
        );
    }

    #[test]
    fn cachelon_telemetry_record_clear() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        telemetry.record("test_cache", CacheOperation::Clear, CacheEvent::Ok, Some(Duration::from_secs(1)));
    }

    #[test]
    fn cachelon_telemetry_record_error() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        telemetry.record(
            "test_cache",
            CacheOperation::Get,
            CacheEvent::Error,
            Some(Duration::from_millis(100)),
        );
    }

    #[test]
    fn cachelon_telemetry_record_without_duration() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Record without duration
        telemetry.record("test_cache", CacheOperation::Get, CacheEvent::Hit, None);
    }

    #[test]
    fn cachelon_telemetry_record_fallback_events() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        telemetry.record(
            "test_cache",
            CacheOperation::Get,
            CacheEvent::Fallback,
            Some(Duration::from_millis(50)),
        );
        telemetry.record(
            "test_cache",
            CacheOperation::Insert,
            CacheEvent::FallbackPromotion,
            Some(Duration::from_millis(25)),
        );
    }

    #[test]
    fn cachelon_telemetry_record_refresh_events() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        telemetry.record(
            "test_cache",
            CacheOperation::Get,
            CacheEvent::RefreshHit,
            Some(Duration::from_millis(30)),
        );
        telemetry.record(
            "test_cache",
            CacheOperation::Get,
            CacheEvent::RefreshMiss,
            Some(Duration::from_millis(40)),
        );
    }

    #[test]
    fn cachelon_telemetry_record_expired() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        telemetry.record(
            "test_cache",
            CacheOperation::Get,
            CacheEvent::Expired,
            Some(Duration::from_millis(2)),
        );
    }

    #[test]
    fn cachelon_telemetry_clone() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        let cloned = telemetry.clone();

        // Both should work
        telemetry.record("cache1", CacheOperation::Get, CacheEvent::Hit, None);
        cloned.record("cache2", CacheOperation::Get, CacheEvent::Miss, None);
    }

    #[test]
    fn cachelon_telemetry_debug() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        let debug_str = format!("{telemetry:?}");
        assert!(debug_str.contains("CacheTelemetry"));
    }

    #[test]
    fn cachelon_telemetry_inner_debug() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Access inner through clone to verify debug
        let debug_str = format!("{telemetry:?}");
        assert!(!debug_str.is_empty());
    }

    #[test]
    fn cachelon_telemetry_record_size() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Record various cache sizes
        telemetry.record_size("test_cache", 0);
        telemetry.record_size("test_cache", 100);
        telemetry.record_size("test_cache", 1000);
    }

    #[test]
    fn cachelon_telemetry_record_size_multiple_caches() {
        let clock = Clock::new_frozen();
        let (logger_provider, meter_provider) = create_test_providers();
        let telemetry = CacheTelemetry::new(logger_provider, &meter_provider, clock);

        // Record sizes for different caches
        telemetry.record_size("cachelon_1", 50);
        telemetry.record_size("cachelon_2", 100);
        telemetry.record_size("cachelon_3", 200);
    }
}
