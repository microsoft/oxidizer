// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cache telemetry types and recording.

use std::cell::Cell;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use pin_project_lite::pin_project;
use tracing::Span;

use crate::cache::CacheName;
use crate::telemetry::attributes;
use crate::telemetry::handler::{CacheEventHandler, CacheOperationEvent, CacheTierEvent, RequestId};

/// Process-wide counter for generating unique request IDs.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

std::thread_local! {
    static CURRENT_REQUEST_ID: Cell<RequestId> = const { Cell::new(0) };
}

/// Generates a unique request ID for correlating tier events with their parent operation.
pub(crate) fn next_request_id() -> RequestId {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

pin_project! {
    /// A future wrapper that restores the request ID into the thread-local
    /// on every poll. This ensures the correct request ID is available
    /// even if the task migrates to a different thread between polls.
    ///
    /// Same pattern as `tracing::Instrument` which re-enters the span per poll.
    pub(crate) struct WithRequestId<F> {
        #[pin]
        inner: F,
        request_id: RequestId,
    }
}

impl<F: Future> Future for WithRequestId<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        CURRENT_REQUEST_ID.with(|cell| cell.set(*this.request_id));
        this.inner.poll(cx)
    }
}

/// Extension trait for wrapping a future with a request ID.
pub(crate) trait WithRequestIdExt: Sized {
    /// Wraps this future so that `request_id` is set in the thread-local
    /// on every poll, surviving task migration across threads.
    fn with_request_id(self, request_id: RequestId) -> WithRequestId<Self>;
}

impl<F: Future> WithRequestIdExt for F {
    fn with_request_id(self, request_id: RequestId) -> WithRequestId<Self> {
        WithRequestId { inner: self, request_id }
    }
}

/// Converts a `Duration` to nanoseconds as `u64`, saturating at `u64::MAX`.
/// A `u64` of nanoseconds covers around 584 years - overflow is not a practical concern.
fn saturating_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// Cache telemetry provider.
///
/// This type is created internally by the cache builder and handles
/// creating cache operation spans and recording structured tracing events.
#[derive(Clone, Default)]
pub struct CacheTelemetry {
    #[cfg(any(feature = "logs", test))]
    logging_enabled: bool,
    handler: Option<Arc<dyn CacheEventHandler>>,
}

impl std::fmt::Debug for CacheTelemetry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheTelemetry")
            .field("logging_enabled", &{
                #[cfg(any(feature = "logs", test))]
                {
                    self.logging_enabled
                }
                #[cfg(not(any(feature = "logs", test)))]
                {
                    false
                }
            })
            .field("has_handler", &self.handler.is_some())
            .finish()
    }
}

impl CacheTelemetry {
    /// Creates a new `CacheTelemetry` with logging disabled.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(any(feature = "logs", test))]
            logging_enabled: false,
            handler: None,
        }
    }

    #[must_use]
    pub(crate) fn with_handler(mut self, handler: Arc<dyn CacheEventHandler>) -> Self {
        self.handler = Some(handler);
        self
    }

    pub(crate) fn current_request_id() -> RequestId {
        CURRENT_REQUEST_ID.with(Cell::get)
    }

    fn emit_tier_event(&self, request_id: RequestId, tier_name: CacheName, outcome: &'static str, duration: Duration, fallback: bool) {
        if let Some(handler) = &self.handler {
            handler.on_tier_event(&CacheTierEvent {
                request_id,
                tier_name,
                outcome,
                duration,
                fallback,
            });
        }
    }

    #[expect(clippy::unused_self, reason = "Consistent API — may use self in future (e.g., emit migration)")]
    fn record_debug_with_duration(&self, event: &'static str, duration: Duration) {
        let span = Span::current();
        if !span.is_disabled() {
            let duration_ns = saturating_nanos(duration);
            span.record(attributes::FIELD_EVENT, event);
            span.record(attributes::FIELD_DURATION_NS, duration_ns);
            tracing::debug!(cache.event = event, cache.duration_ns = duration_ns);
        }
    }

    #[expect(clippy::unused_self, reason = "Consistent API — may use self in future (e.g., emit migration)")]
    fn record_info_with_duration(&self, event: &'static str, duration: Duration) {
        let span = Span::current();
        if !span.is_disabled() {
            let duration_ns = saturating_nanos(duration);
            span.record(attributes::FIELD_EVENT, event);
            span.record(attributes::FIELD_DURATION_NS, duration_ns);
            tracing::info!(cache.event = event, cache.duration_ns = duration_ns);
        }
    }

    #[expect(clippy::unused_self, reason = "Consistent API — may use self in future (e.g., emit migration)")]
    fn record_error_with_duration(&self, event: &'static str, duration: Duration) {
        let span = Span::current();
        if !span.is_disabled() {
            let duration_ns = saturating_nanos(duration);
            span.record(attributes::FIELD_EVENT, event);
            span.record(attributes::FIELD_DURATION_NS, duration_ns);
            tracing::error!(cache.event = event, cache.duration_ns = duration_ns);
        }
    }

    #[expect(clippy::unused_self, reason = "Consistent API — may use self in future (e.g., emit migration)")]
    fn record_info_event(&self, event: &'static str) {
        let span = Span::current();
        if !span.is_disabled() {
            span.record(attributes::FIELD_EVENT, event);
            tracing::info!(cache.event = event);
        }
    }

    pub(crate) fn record_hit(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_debug_with_duration(attributes::EVENT_HIT, duration);
        self.emit_tier_event(Self::current_request_id(), tier_name, attributes::EVENT_HIT, duration, fallback);
    }

    pub(crate) fn record_miss(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_debug_with_duration(attributes::EVENT_MISS, duration);
        self.emit_tier_event(Self::current_request_id(), tier_name, attributes::EVENT_MISS, duration, fallback);
    }

    pub(crate) fn record_expired(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_info_with_duration(attributes::EVENT_EXPIRED, duration);
        self.emit_tier_event(Self::current_request_id(), tier_name, attributes::EVENT_EXPIRED, duration, fallback);
    }

    pub(crate) fn record_get_error(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_error_with_duration(attributes::EVENT_GET_ERROR, duration);
        self.emit_tier_event(
            Self::current_request_id(),
            tier_name,
            attributes::EVENT_GET_ERROR,
            duration,
            fallback,
        );
    }

    pub(crate) fn record_inserted(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_info_with_duration(attributes::EVENT_INSERTED, duration);
        self.emit_tier_event(
            Self::current_request_id(),
            tier_name,
            attributes::EVENT_INSERTED,
            duration,
            fallback,
        );
    }

    pub(crate) fn record_insert_error(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_error_with_duration(attributes::EVENT_INSERT_ERROR, duration);
        self.emit_tier_event(
            Self::current_request_id(),
            tier_name,
            attributes::EVENT_INSERT_ERROR,
            duration,
            fallback,
        );
    }

    pub(crate) fn record_invalidated(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_info_with_duration(attributes::EVENT_INVALIDATED, duration);
        self.emit_tier_event(
            Self::current_request_id(),
            tier_name,
            attributes::EVENT_INVALIDATED,
            duration,
            fallback,
        );
    }

    pub(crate) fn record_invalidate_error(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_error_with_duration(attributes::EVENT_INVALIDATE_ERROR, duration);
        self.emit_tier_event(
            Self::current_request_id(),
            tier_name,
            attributes::EVENT_INVALIDATE_ERROR,
            duration,
            fallback,
        );
    }

    pub(crate) fn record_cleared(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_debug_with_duration(attributes::EVENT_CLEARED, duration);
        self.emit_tier_event(Self::current_request_id(), tier_name, attributes::EVENT_CLEARED, duration, fallback);
    }

    pub(crate) fn record_clear_error(&self, tier_name: CacheName, duration: Duration, fallback: bool) {
        self.record_error_with_duration(attributes::EVENT_CLEAR_ERROR, duration);
        self.emit_tier_event(
            Self::current_request_id(),
            tier_name,
            attributes::EVENT_CLEAR_ERROR,
            duration,
            fallback,
        );
    }

    pub(crate) fn record_refresh_hit(&self, duration: Duration) {
        self.record_debug_with_duration(attributes::EVENT_REFRESH_HIT, duration);
    }

    pub(crate) fn record_refresh_miss(&self, duration: Duration) {
        self.record_info_with_duration(attributes::EVENT_REFRESH_MISS, duration);
    }

    pub(crate) fn record_insert_rejected(&self, tier_name: CacheName, fallback: bool) {
        self.record_info_event(attributes::EVENT_INSERT_REJECTED);
        self.emit_tier_event(
            Self::current_request_id(),
            tier_name,
            attributes::EVENT_INSERT_REJECTED,
            Duration::ZERO,
            fallback,
        );
    }

    #[expect(clippy::unused_self, reason = "Consistent API — may use self in future (e.g., emit migration)")]
    pub(crate) fn record_fallback(&self) {
        let span = Span::current();
        if !span.is_disabled() {
            span.record(attributes::FIELD_FALLBACK, true);
        }
    }

    /// Records that an entry was evicted from the cache due to capacity limits.
    ///
    /// When moka evicts during an `insert()`, the eviction listener runs
    /// synchronously on the inserting thread, so the thread-local request ID
    /// is still set. This allows correlating capacity evictions with the
    /// insert that caused them. Background maintenance evictions will have
    /// a request ID of 0.
    #[cfg(any(feature = "memory", test))]
    pub(crate) fn record_eviction(&self, cache_name: CacheName) {
        #[cfg(any(feature = "logs", test))]
        if self.logging_enabled {
            tracing::info!(cache.name = cache_name, cache.event = attributes::EVENT_EVICTION);
        }

        self.emit_tier_event(
            Self::current_request_id(),
            cache_name,
            attributes::EVENT_EVICTION,
            Duration::ZERO,
            false,
        );
    }

    /// Records that an entry expired in the background (moka eviction listener).
    ///
    /// Unlike [`record_expired`](Self::record_expired), this fires from a
    /// background thread with no parent span, so it emits a standalone event.
    /// Like [`record_eviction`](Self::record_eviction), the request ID is
    /// read from the thread-local (non-zero when triggered synchronously
    /// during a cache operation).
    #[cfg(feature = "memory")]
    pub(crate) fn record_background_expired(&self, cache_name: CacheName) {
        #[cfg(any(feature = "logs", test))]
        if self.logging_enabled {
            tracing::debug!(cache.name = cache_name, cache.event = attributes::EVENT_EXPIRED);
        }

        self.emit_tier_event(
            Self::current_request_id(),
            cache_name,
            attributes::EVENT_EXPIRED,
            Duration::ZERO,
            false,
        );
    }

    pub(crate) fn complete_operation(
        &self,
        request_id: RequestId,
        cache_name: CacheName,
        operation: &'static str,
        duration: Duration,
        coalesced: bool,
    ) {
        let span = Span::current();
        if !span.is_disabled() {
            span.record(attributes::FIELD_DURATION_NS, saturating_nanos(duration));
            if coalesced {
                span.record(attributes::FIELD_COALESCED, true);
            }
        }

        if let Some(handler) = &self.handler {
            handler.on_operation_complete(&CacheOperationEvent {
                request_id,
                cache_name,
                operation,
                duration,
                coalesced,
            });
        }
    }
}

#[cfg(any(feature = "logs", test))]
impl CacheTelemetry {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_logging() -> Self {
        Self::new().enable_logging()
    }

    #[must_use]
    pub(crate) fn enable_logging(mut self) -> Self {
        self.logging_enabled = true;
        self
    }

    pub(crate) fn get_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.get",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty,
                cache.coalesced = tracing::field::Empty,
                cache.fallback = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }

    pub(crate) fn insert_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.insert",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }

    pub(crate) fn invalidate_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.invalidate",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }

    pub(crate) fn clear_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.clear",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }

    pub(crate) fn get_or_insert_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.get_or_insert",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty,
                cache.coalesced = tracing::field::Empty,
                cache.fallback = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }

    pub(crate) fn try_get_or_insert_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.try_get_or_insert",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty,
                cache.coalesced = tracing::field::Empty,
                cache.fallback = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }

    pub(crate) fn optionally_get_or_insert_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.optionally_get_or_insert",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty,
                cache.coalesced = tracing::field::Empty,
                cache.fallback = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }

    pub(crate) fn tier_span(&self, name: CacheName) -> Span {
        if self.logging_enabled {
            tracing::span!(
                tracing::Level::DEBUG,
                "cache.tier",
                cache.name = name,
                cache.event = tracing::field::Empty,
                cache.duration_ns = tracing::field::Empty
            )
        } else {
            Span::none()
        }
    }
}

#[cfg(not(any(feature = "logs", test)))]
#[cfg_attr(test, mutants::skip)] // Equivalent mutants: cfg-gated off during testing.
#[expect(clippy::unused_self, reason = "Span factories are no-ops when logs are disabled")]
impl CacheTelemetry {
    pub(crate) fn get_span(&self, _: CacheName) -> Span {
        Span::none()
    }

    pub(crate) fn insert_span(&self, _: CacheName) -> Span {
        Span::none()
    }

    pub(crate) fn invalidate_span(&self, _: CacheName) -> Span {
        Span::none()
    }

    pub(crate) fn clear_span(&self, _: CacheName) -> Span {
        Span::none()
    }

    pub(crate) fn get_or_insert_span(&self, _: CacheName) -> Span {
        Span::none()
    }

    pub(crate) fn try_get_or_insert_span(&self, _: CacheName) -> Span {
        Span::none()
    }

    pub(crate) fn optionally_get_or_insert_span(&self, _: CacheName) -> Span {
        Span::none()
    }

    pub(crate) fn tier_span(&self, _: CacheName) -> Span {
        Span::none()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use testing_aids::LogCapture;
    use tracing::Instrument;
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    fn subscriber(capture: &LogCapture) -> impl tracing::Subscriber {
        tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_writer(capture.clone())
                .with_ansi(false)
                .with_span_events(FmtSpan::CLOSE),
        )
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn logs_emit_contains_all_fields_and_values() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(subscriber(&capture));
        let telemetry = CacheTelemetry::with_logging();

        let request_id = next_request_id();
        futures::executor::block_on(
            async {
                telemetry.record_hit("my_test_cache", Duration::from_nanos(12345), false);
                telemetry.complete_operation(request_id, "my_test_cache", "cache.get", Duration::from_nanos(12345), true);
                telemetry.record_fallback();
            }
            .with_request_id(request_id)
            .instrument(telemetry.get_span("my_test_cache")),
        );

        capture.assert_contains(attributes::FIELD_NAME);
        capture.assert_contains(attributes::FIELD_EVENT);
        capture.assert_contains(attributes::FIELD_DURATION_NS);
        capture.assert_contains(attributes::FIELD_COALESCED);
        capture.assert_contains(attributes::FIELD_FALLBACK);
        capture.assert_contains("my_test_cache");
        capture.assert_contains(attributes::EVENT_HIT);
        capture.assert_contains("12345");
        capture.assert_contains("true");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn logs_emit_at_correct_severity_levels() {
        let telemetry = CacheTelemetry::with_logging();

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(subscriber(&capture));
        let request_id = next_request_id();
        futures::executor::block_on(
            async { telemetry.record_get_error("cache", Duration::ZERO, false) }
                .with_request_id(request_id)
                .instrument(telemetry.tier_span("cache")),
        );
        capture.assert_contains("ERROR");

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(subscriber(&capture));
        let request_id = next_request_id();
        futures::executor::block_on(
            async { telemetry.record_expired("cache", Duration::ZERO, false) }
                .with_request_id(request_id)
                .instrument(telemetry.tier_span("cache")),
        );
        capture.assert_contains("INFO");

        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(subscriber(&capture));
        let request_id = next_request_id();
        futures::executor::block_on(
            async { telemetry.record_hit("cache", Duration::ZERO, false) }
                .with_request_id(request_id)
                .instrument(telemetry.tier_span("cache")),
        );
        capture.assert_contains("DEBUG");
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn all_span_factories_produce_enabled_spans() {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(subscriber(&capture));
        let t = CacheTelemetry::with_logging();

        // Exercise every span factory so coverage includes them all.
        let spans = [
            t.get_span("c"),
            t.insert_span("c"),
            t.invalidate_span("c"),
            t.clear_span("c"),
            t.get_or_insert_span("c"),
            t.try_get_or_insert_span("c"),
            t.optionally_get_or_insert_span("c"),
            t.tier_span("c"),
        ];

        for span in &spans {
            assert!(!span.is_disabled(), "span factory should produce an enabled span");
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn telemetry_disabled_emits_nothing() {
        let telemetry = CacheTelemetry::new();
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(subscriber(&capture));

        let request_id = next_request_id();
        futures::executor::block_on(
            async { telemetry.record_hit("cache", Duration::from_secs(1), false) }
                .with_request_id(request_id)
                .instrument(telemetry.get_span("cache")),
        );

        assert!(capture.output().is_empty());
    }

    #[cfg_attr(miri, ignore)]
    fn assert_emits(expected: &str, f: impl FnOnce(&CacheTelemetry, RequestId)) {
        let capture = LogCapture::new();
        let _guard = tracing::subscriber::set_default(subscriber(&capture));
        let telemetry = CacheTelemetry::with_logging();
        let request_id = next_request_id();
        f(&telemetry, request_id);
        capture.assert_contains(expected);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn every_helper_emits_its_event() {
        assert_emits(attributes::EVENT_HIT, |t, request_id| {
            futures::executor::block_on(
                async { t.record_hit("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.tier_span("c")),
            );
        });
        assert_emits(attributes::EVENT_MISS, |t, request_id| {
            futures::executor::block_on(
                async { t.record_miss("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.tier_span("c")),
            );
        });
        assert_emits(attributes::EVENT_EXPIRED, |t, request_id| {
            futures::executor::block_on(
                async { t.record_expired("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.tier_span("c")),
            );
        });
        assert_emits(attributes::EVENT_GET_ERROR, |t, request_id| {
            futures::executor::block_on(
                async { t.record_get_error("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.tier_span("c")),
            );
        });
        assert_emits(attributes::EVENT_REFRESH_HIT, |t, request_id| {
            futures::executor::block_on(
                async { t.record_refresh_hit(Duration::ZERO) }
                    .with_request_id(request_id)
                    .instrument(t.get_span("c")),
            );
        });
        assert_emits(attributes::EVENT_REFRESH_MISS, |t, request_id| {
            futures::executor::block_on(
                async { t.record_refresh_miss(Duration::ZERO) }
                    .with_request_id(request_id)
                    .instrument(t.get_span("c")),
            );
        });
        assert_emits(attributes::EVENT_INSERTED, |t, request_id| {
            futures::executor::block_on(
                async { t.record_inserted("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.insert_span("c")),
            );
        });
        assert_emits(attributes::EVENT_INSERT_REJECTED, |t, request_id| {
            futures::executor::block_on(
                async { t.record_insert_rejected("c", false) }
                    .with_request_id(request_id)
                    .instrument(t.insert_span("c")),
            );
        });
        assert_emits(attributes::EVENT_INSERT_ERROR, |t, request_id| {
            futures::executor::block_on(
                async { t.record_insert_error("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.insert_span("c")),
            );
        });
        assert_emits(attributes::EVENT_INVALIDATED, |t, request_id| {
            futures::executor::block_on(
                async { t.record_invalidated("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.invalidate_span("c")),
            );
        });
        assert_emits(attributes::EVENT_INVALIDATE_ERROR, |t, request_id| {
            futures::executor::block_on(
                async { t.record_invalidate_error("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.invalidate_span("c")),
            );
        });
        assert_emits(attributes::EVENT_CLEARED, |t, request_id| {
            futures::executor::block_on(
                async { t.record_cleared("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.clear_span("c")),
            );
        });
        assert_emits(attributes::EVENT_CLEAR_ERROR, |t, request_id| {
            futures::executor::block_on(
                async { t.record_clear_error("c", Duration::ZERO, false) }
                    .with_request_id(request_id)
                    .instrument(t.clear_span("c")),
            );
        });
        assert_emits(attributes::EVENT_EVICTION, |t, request_id| {
            futures::executor::block_on(
                async { t.record_eviction("c") }
                    .with_request_id(request_id)
                    .instrument(t.get_span("c")),
            );
        });
    }

    #[test]
    fn handler_receives_tier_and_operation_events_without_logging() {
        type EventRecord = Vec<(RequestId, String, String, u128, bool)>;

        #[derive(Clone)]
        struct RecordingHandler {
            tier_events: Arc<Mutex<EventRecord>>,
            operation_events: Arc<Mutex<EventRecord>>,
        }

        impl CacheEventHandler for RecordingHandler {
            fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
                self.tier_events.lock().expect("test handler mutex should not be poisoned").push((
                    event.request_id,
                    event.tier_name.to_string(),
                    event.outcome.to_string(),
                    event.duration.as_nanos(),
                    event.fallback,
                ));
            }

            fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
                self.operation_events
                    .lock()
                    .expect("test handler mutex should not be poisoned")
                    .push((
                        event.request_id,
                        event.cache_name.to_string(),
                        event.operation.to_string(),
                        event.duration.as_nanos(),
                        event.coalesced,
                    ));
            }
        }

        let tier_events = Arc::new(Mutex::new(Vec::new()));
        let operation_events = Arc::new(Mutex::new(Vec::new()));
        let telemetry = CacheTelemetry::new().with_handler(Arc::new(RecordingHandler {
            tier_events: Arc::clone(&tier_events),
            operation_events: Arc::clone(&operation_events),
        }));

        let request_id = next_request_id();
        futures::executor::block_on(
            async {
                telemetry.record_hit("l2", Duration::from_nanos(7), true);
                telemetry.complete_operation(request_id, "cache", "cache.get", Duration::from_nanos(11), true);
            }
            .with_request_id(request_id),
        );

        assert_eq!(
            *tier_events.lock().expect("test handler mutex should not be poisoned"),
            vec![(request_id, "l2".to_string(), attributes::EVENT_HIT.to_string(), 7, true)]
        );
        assert_eq!(
            *operation_events.lock().expect("test handler mutex should not be poisoned"),
            vec![(request_id, "cache".to_string(), "cache.get".to_string(), 11, true)]
        );
    }

    #[test]
    fn next_request_id_returns_unique_incrementing_values() {
        let a = next_request_id();
        let b = next_request_id();
        let c = next_request_id();
        assert!(b > a, "request IDs must increment: got {a} then {b}");
        assert!(c > b, "request IDs must increment: got {b} then {c}");
    }

    #[test]
    fn eviction_handler_receives_request_id_from_calling_thread() {
        type TierRecord = (RequestId, String, String);
        type OpRecord = (RequestId, String, String);

        struct EvictionRecorder {
            tier_events: Arc<Mutex<Vec<TierRecord>>>,
            operation_events: Arc<Mutex<Vec<OpRecord>>>,
        }
        impl CacheEventHandler for EvictionRecorder {
            fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
                self.tier_events.lock().expect("test mutex should not be poisoned").push((
                    event.request_id,
                    event.tier_name.to_string(),
                    event.outcome.to_string(),
                ));
            }
            fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
                self.operation_events.lock().expect("test mutex should not be poisoned").push((
                    event.request_id,
                    event.cache_name.to_string(),
                    event.operation.to_string(),
                ));
            }
        }

        let tier_events = Arc::new(Mutex::new(Vec::new()));
        let operation_events = Arc::new(Mutex::new(Vec::new()));
        let telemetry = CacheTelemetry::new().with_handler(Arc::new(EvictionRecorder {
            tier_events: Arc::clone(&tier_events),
            operation_events: Arc::clone(&operation_events),
        }));

        let request_id = next_request_id();
        futures::executor::block_on(
            async {
                telemetry.record_eviction("my_cache");
                telemetry.complete_operation(request_id, "my_cache", "cache.insert", Duration::ZERO, false);
            }
            .with_request_id(request_id),
        );

        let tiers = tier_events.lock().expect("test mutex should not be poisoned");
        assert_eq!(tiers.len(), 1, "expected exactly one eviction tier event");
        assert_eq!(tiers[0].0, request_id, "eviction should carry the inserting thread's request_id");
        assert_eq!(tiers[0].2, attributes::EVENT_EVICTION);

        let ops = operation_events.lock().expect("test mutex should not be poisoned");
        assert_eq!(ops.len(), 1, "expected one operation complete event");
        assert_eq!(ops[0].0, request_id);
        assert_eq!(ops[0].2, "cache.insert");
    }

    #[test]
    fn eviction_without_request_context_has_zero_id() {
        type TierRecord = (RequestId, String);
        type OpRecord = (RequestId, String);

        struct IdRecorder {
            tier_events: Arc<Mutex<Vec<TierRecord>>>,
            operation_events: Arc<Mutex<Vec<OpRecord>>>,
        }
        impl CacheEventHandler for IdRecorder {
            fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
                self.tier_events
                    .lock()
                    .expect("test mutex should not be poisoned")
                    .push((event.request_id, event.outcome.to_string()));
            }
            fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
                self.operation_events
                    .lock()
                    .expect("test mutex should not be poisoned")
                    .push((event.request_id, event.operation.to_string()));
            }
        }

        let tier_events = Arc::new(Mutex::new(Vec::new()));
        let operation_events = Arc::new(Mutex::new(Vec::new()));
        let telemetry = CacheTelemetry::new().with_handler(Arc::new(IdRecorder {
            tier_events: Arc::clone(&tier_events),
            operation_events: Arc::clone(&operation_events),
        }));

        // No WithRequestId wrapper — simulates background maintenance thread
        telemetry.record_eviction("bg_cache");
        telemetry.complete_operation(0, "bg_cache", "background", Duration::ZERO, false);

        let tiers = tier_events.lock().expect("test mutex should not be poisoned");
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].0, 0, "background eviction should have request_id 0");

        let ops = operation_events.lock().expect("test mutex should not be poisoned");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].0, 0);
    }
}
