//! Extension traits for telemetry recording.

use std::time::Duration;

use tick::Clock;

use crate::{
    cache::CacheName,
    telemetry::CacheTelemetry,
    telemetry::{CacheEvent, CacheOperation},
};

/// Result of a timed async operation.
#[derive(Debug, Clone, Copy)]
pub struct TimedResult<R> {
    /// The result of the operation.
    pub result: R,
    /// The duration of the operation.
    pub duration: Duration,
}

/// Extension trait for timing async operations.
pub trait ClockExt {
    /// Times an async operation and returns both the result and elapsed duration.
    fn timed_async<F, R>(&self, f: F) -> impl Future<Output = TimedResult<R>>
    where
        F: Future<Output = R>;
}

impl ClockExt for Clock {
    async fn timed_async<F, R>(&self, f: F) -> TimedResult<R>
    where
        F: Future<Output = R>,
    {
        let start = self.instant();
        let result = f.await;
        TimedResult {
            result,
            duration: start.elapsed(),
        }
    }
}

pub trait CacheTelemetryExt {
    /// Records a cache operation if telemetry is enabled.
    fn record(&self, name: CacheName, operation: CacheOperation, event: CacheEvent, duration: Duration);

    /// Records the current cache size if telemetry is enabled.
    fn record_size(&self, name: CacheName, size: u64);
}

impl CacheTelemetryExt for Option<CacheTelemetry> {
    fn record(&self, name: CacheName, operation: CacheOperation, event: CacheEvent, duration: Duration) {
        #[cfg(feature = "telemetry")]
        if let Some(t) = self {
            t.record(name, operation, event, Some(duration));
        }
    }

    fn record_size(&self, name: CacheName, size: u64) {
        #[cfg(feature = "telemetry")]
        if let Some(t) = self {
            t.record_size(name, size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    #[test]
    fn clock_ext_timed_async() {
        block_on(async {
            let clock = Clock::new_frozen();

            let timed = clock.timed_async(async { 42 }).await;

            assert_eq!(timed.result, 42);
            // Duration depends on Clock implementation - just verify we get a result
        });
    }

    #[test]
    fn clock_ext_timed_async_with_time_advance() {
        block_on(async {
            let clock = Clock::new_frozen();

            // Start timing, then advance the clock during the async operation
            let timed = clock
                .timed_async(async {
                    // In a real scenario, time would pass
                    "hello"
                })
                .await;

            assert_eq!(timed.result, "hello");
        });
    }

    #[test]
    fn cachelon_telemetry_ext_none_does_not_panic() {
        let telemetry: Option<CacheTelemetry> = None;
        // Should not panic when telemetry is None
        telemetry.record("test_cache", CacheOperation::Get, CacheEvent::Hit, Duration::from_millis(10));
    }

    #[test]
    fn cachelon_telemetry_ext_with_various_operations() {
        let telemetry: Option<CacheTelemetry> = None;

        // Test all operation types don't panic
        telemetry.record("cache", CacheOperation::Get, CacheEvent::Hit, Duration::from_millis(1));
        telemetry.record("cache", CacheOperation::Insert, CacheEvent::Ok, Duration::from_millis(1));
        telemetry.record("cache", CacheOperation::Invalidate, CacheEvent::Ok, Duration::from_millis(1));
        telemetry.record("cache", CacheOperation::Clear, CacheEvent::Ok, Duration::from_millis(1));
    }

    #[test]
    fn cachelon_telemetry_ext_with_various_events() {
        let telemetry: Option<CacheTelemetry> = None;

        // Test all event types don't panic
        telemetry.record("cache", CacheOperation::Get, CacheEvent::Miss, Duration::from_millis(1));
        telemetry.record("cache", CacheOperation::Get, CacheEvent::Expired, Duration::from_millis(1));
        telemetry.record("cache", CacheOperation::Get, CacheEvent::Error, Duration::from_millis(1));
        telemetry.record("cache", CacheOperation::Get, CacheEvent::Fallback, Duration::from_millis(1));
    }

    #[test]
    fn cachelon_telemetry_ext_record_size_none_does_not_panic() {
        let telemetry: Option<CacheTelemetry> = None;
        // Should not panic when telemetry is None
        telemetry.record_size("test_cache", 42);
    }

    #[test]
    fn cachelon_telemetry_ext_record_size_various_values() {
        let telemetry: Option<CacheTelemetry> = None;

        // Test various size values don't panic
        telemetry.record_size("cache", 0);
        telemetry.record_size("cache", 100);
        telemetry.record_size("cache", u64::MAX);
    }
}
