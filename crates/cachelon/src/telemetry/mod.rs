// Copyright (c) Microsoft Corporation.

//! Cache telemetry integration with OpenTelemetry.
//!
//! This module provides telemetry recording for cache operations using
//! OpenTelemetry metrics and logs. When the `telemetry` feature is enabled,
//! all cache operations emit structured logs and metrics.

#[cfg(feature = "telemetry")]
use cache::CacheTelemetryInner;
#[cfg(feature = "telemetry")]
use opentelemetry::logs::Severity;

use thread_aware::{Arc, PerCore};

#[cfg(feature = "telemetry")]
pub(crate) mod cache;
pub(crate) mod ext;

/// Cache telemetry provider for OpenTelemetry integration.
///
/// This type wraps OpenTelemetry logger and meter providers, enabling
/// automatic recording of cache operations as structured logs and metrics.
///
/// Construct this and pass it to the cache builder via `.telemetry()`.
#[derive(Clone, Debug)]
pub struct CacheTelemetry {
    #[cfg(feature = "telemetry")]
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
            Self::Get => "get",
            Self::Insert => "insert",
            Self::Invalidate => "invalidate",
            Self::Clear => "clear",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CacheEvent {
    Hit,
    Expired,
    Miss,
    RefreshHit,
    RefreshMiss,
    Inserted,
    Invalidated,
    Ok,
    #[cfg_attr(not(test), expect(dead_code, reason = "Reserved for future telemetry integration"))]
    Fallback,
    FallbackPromotion,
    Error,
}

impl CacheEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Expired => "expired",
            Self::Miss => "miss",
            Self::RefreshHit => "refresh_hit",
            Self::RefreshMiss => "refresh_miss",
            Self::Inserted => "inserted",
            Self::Invalidated => "invalidated",
            Self::Ok => "ok",
            Self::Fallback => "fallback",
            Self::FallbackPromotion => "fallback_promotion",
            Self::Error => "error",
        }
    }

    #[cfg(feature = "telemetry")]
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
    fn cachelon_operation_as_str() {
        assert_eq!(CacheOperation::Get.as_str(), "get");
        assert_eq!(CacheOperation::Insert.as_str(), "insert");
        assert_eq!(CacheOperation::Invalidate.as_str(), "invalidate");
        assert_eq!(CacheOperation::Clear.as_str(), "clear");
    }

    #[test]
    fn cachelon_operation_debug() {
        let get = CacheOperation::Get;
        let debug_str = format!("{get:?}");
        assert!(debug_str.contains("Get"));
    }

    #[test]
    fn cachelon_operation_clone() {
        let get = CacheOperation::Get;
        let cloned = get;
        assert_eq!(get.as_str(), cloned.as_str());
    }

    #[test]
    fn cachelon_event_as_str() {
        assert_eq!(CacheEvent::Hit.as_str(), "hit");
        assert_eq!(CacheEvent::Expired.as_str(), "expired");
        assert_eq!(CacheEvent::Miss.as_str(), "miss");
        assert_eq!(CacheEvent::RefreshHit.as_str(), "refresh_hit");
        assert_eq!(CacheEvent::RefreshMiss.as_str(), "refresh_miss");
        assert_eq!(CacheEvent::Inserted.as_str(), "inserted");
        assert_eq!(CacheEvent::Invalidated.as_str(), "invalidated");
        assert_eq!(CacheEvent::Ok.as_str(), "ok");
        assert_eq!(CacheEvent::Fallback.as_str(), "fallback");
        assert_eq!(CacheEvent::FallbackPromotion.as_str(), "fallback_promotion");
        assert_eq!(CacheEvent::Error.as_str(), "error");
    }

    #[test]
    fn cachelon_event_debug() {
        let hit = CacheEvent::Hit;
        let debug_str = format!("{hit:?}");
        assert!(debug_str.contains("Hit"));
    }

    #[test]
    fn cachelon_event_clone() {
        let hit = CacheEvent::Hit;
        let cloned = hit;
        assert_eq!(hit.as_str(), cloned.as_str());
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn cachelon_event_severity_debug() {
        assert_eq!(CacheEvent::Hit.severity(), Severity::Debug);
        assert_eq!(CacheEvent::Miss.severity(), Severity::Debug);
        assert_eq!(CacheEvent::RefreshHit.severity(), Severity::Debug);
        assert_eq!(CacheEvent::Ok.severity(), Severity::Debug);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn cachelon_event_severity_info() {
        assert_eq!(CacheEvent::Expired.severity(), Severity::Info);
        assert_eq!(CacheEvent::RefreshMiss.severity(), Severity::Info);
        assert_eq!(CacheEvent::Inserted.severity(), Severity::Info);
        assert_eq!(CacheEvent::Invalidated.severity(), Severity::Info);
        assert_eq!(CacheEvent::Fallback.severity(), Severity::Info);
        assert_eq!(CacheEvent::FallbackPromotion.severity(), Severity::Info);
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn cachelon_event_severity_error() {
        assert_eq!(CacheEvent::Error.severity(), Severity::Error);
    }
}
