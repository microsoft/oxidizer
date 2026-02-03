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
use opentelemetry::logs::Severity;

use thread_aware::{Arc, PerCore};

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

#[derive(Debug, Clone, Copy)]
pub(crate) enum CacheOperation {
    Get,
    Insert,
    Invalidate,
    Clear,
}

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

    #[cfg(any(feature = "logs", feature = "metrics", test))]
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
    fn cache_operation_debug() {
        let get = CacheOperation::Get;
        let debug_str = format!("{get:?}");
        assert!(debug_str.contains("Get"));
    }

    #[test]
    fn cache_operation_clone() {
        let get = CacheOperation::Get;
        let cloned = get;
        assert_eq!(get.as_str(), cloned.as_str());
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

    #[test]
    fn cache_event_debug() {
        let hit = CacheActivity::Hit;
        let debug_str = format!("{hit:?}");
        assert!(debug_str.contains("Hit"));
    }

    #[test]
    fn cache_event_clone() {
        let hit = CacheActivity::Hit;
        let cloned = hit;
        assert_eq!(hit.as_str(), cloned.as_str());
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
