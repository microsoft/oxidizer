// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry integration with OpenTelemetry.
//!
//! This module provides telemetry recording for cache operations using
//! OpenTelemetry metrics and logs. When the `telemetry` feature is enabled,
//! all cache operations emit structured logs and metrics.

#[cfg(any(feature = "logs", feature = "metrics", test))]
use cache::CacheTelemetryInner;
#[cfg(any(feature = "logs", feature = "metrics", test))]
use opentelemetry::{logs::Severity, metrics::Meter};
#[cfg(any(feature = "logs", feature = "metrics", test))]
use thread_aware::{Arc, PerCore};
#[cfg(any(feature = "logs", feature = "metrics", test))]
use tick::Clock;

#[cfg(any(feature = "logs", feature = "metrics", test))]
pub(crate) mod attributes;
#[cfg(any(feature = "logs", feature = "metrics", test))]
pub(crate) mod cache;
pub(crate) mod ext;
#[cfg(any(feature = "metrics", test))]
pub(crate) mod metrics;
#[cfg(test)]
pub(crate) mod testing;

/// Cache telemetry provider for OpenTelemetry integration.
///
/// This type wraps OpenTelemetry logger and meter providers, enabling
/// automatic recording of cache operations as structured logs and metrics.
///
/// Construct this and pass it to the cache builder via `.telemetry()`.
#[derive(Clone, Debug)]
pub struct CacheTelemetry {
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    inner: Arc<CacheTelemetryInner, PerCore>,
}

impl CacheTelemetry {
    /// Creates a new cache telemetry collector.
    ///
    /// # Arguments
    ///
    /// * `logging_enabled` - Whether logging is enabled
    /// * `meter` - The OpenTelemetry meter to use for metrics
    /// * `clock` - The clock to use for timing events
    #[cfg(any(feature = "logs", feature = "metrics", test))]
    #[must_use]
    pub fn new(logging_enabled: bool, meter: Option<&Meter>, clock: Clock) -> Self {
        use crate::telemetry::metrics::{create_cache_size_gauge, create_event_counter, create_operation_duration_histogram};

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
}

#[cfg(any(feature = "logs", feature = "metrics", test))]
macro_rules! create_telemetry {
    ($builder:expr, $clock:expr) => {
        Some($crate::telemetry::CacheTelemetry::new(
            $builder.logs_enabled,
            $builder.meter.as_ref(),
            $clock,
        ))
    };
}

#[cfg(not(any(feature = "logs", feature = "metrics", test)))]
macro_rules! create_telemetry {
    ($builder:expr, $clock:expr) => {{
        let _ = (&$builder, &$clock); // silence unused warnings
        None::<$crate::telemetry::CacheTelemetry>
    }};
}

pub(crate) use create_telemetry;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_operation_as_str() {
        assert_eq!(CacheOperation::Get.as_str(), "cache.get");
        assert_eq!(CacheOperation::Insert.as_str(), "cache.insert");
        assert_eq!(CacheOperation::Invalidate.as_str(), "cache.invalidate");
        assert_eq!(CacheOperation::Clear.as_str(), "cache.clear");
    }

    #[test]
    fn cache_event_as_str() {
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

    #[cfg(any(feature = "logs", feature = "metrics", test))]
    #[test]
    fn cache_event_severity_debug() {
        assert_eq!(CacheActivity::Hit.severity(), Severity::Debug);
        assert_eq!(CacheActivity::Miss.severity(), Severity::Debug);
        assert_eq!(CacheActivity::RefreshHit.severity(), Severity::Debug);
        assert_eq!(CacheActivity::Ok.severity(), Severity::Debug);
    }

    #[cfg(any(feature = "logs", feature = "metrics", test))]
    #[test]
    fn cache_event_severity_info() {
        assert_eq!(CacheActivity::Expired.severity(), Severity::Info);
        assert_eq!(CacheActivity::RefreshMiss.severity(), Severity::Info);
        assert_eq!(CacheActivity::Inserted.severity(), Severity::Info);
        assert_eq!(CacheActivity::Invalidated.severity(), Severity::Info);
        assert_eq!(CacheActivity::Fallback.severity(), Severity::Info);
        assert_eq!(CacheActivity::FallbackPromotion.severity(), Severity::Info);
    }

    #[cfg(any(feature = "logs", feature = "metrics", test))]
    #[test]
    fn cache_event_severity_error() {
        assert_eq!(CacheActivity::Error.severity(), Severity::Error);
    }
}
