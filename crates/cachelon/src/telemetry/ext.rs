// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Extension traits for telemetry recording.

use std::time::Duration;

use tick::Clock;

use crate::{
    cache::CacheName,
    telemetry::CacheTelemetry,
    telemetry::{CacheActivity, CacheOperation},
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
            duration: self.instant().saturating_duration_since(start),
        }
    }
}

pub trait CacheTelemetryExt {
    /// Records a cache operation if telemetry is enabled.
    fn record(&self, name: CacheName, operation: CacheOperation, event: CacheActivity, duration: Duration);

    /// Records the current cache size if telemetry is enabled.
    fn record_size(&self, name: CacheName, size: u64);
}

impl CacheTelemetryExt for Option<CacheTelemetry> {
    #[allow(unused_variables, reason = "No-op when telemetry is disabled")]
    fn record(&self, name: CacheName, operation: CacheOperation, event: CacheActivity, duration: Duration) {
        #[cfg(any(feature = "logs", feature = "metrics", test))]
        if let Some(t) = self {
            t.record(name, operation, event, Some(duration));
        }
    }

    #[allow(unused_variables, reason = "No-op when telemetry is disabled")]
    fn record_size(&self, name: CacheName, size: u64) {
        #[cfg(any(feature = "logs", feature = "metrics", test))]
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
    fn clock_ext_timed_async_measures_duration() {
        block_on(async {
            let control = tick::ClockControl::new();
            let clock = control.to_clock();

            let timed = clock
                .timed_async(async {
                    control.advance(Duration::from_millis(100));
                    42
                })
                .await;

            assert_eq!(timed.result, 42);
            assert_eq!(timed.duration, Duration::from_millis(100));
        });
    }

    #[test]
    fn telemetry_ext_none_emits_no_logs() {
        use crate::telemetry::testing::LogCapture;

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(capture.subscriber());

        let telemetry: Option<CacheTelemetry> = None;
        telemetry.record("cache", CacheOperation::Get, CacheActivity::Hit, Duration::from_millis(1));
        telemetry.record_size("cache", 42);

        assert!(capture.output().is_empty());
    }
}
