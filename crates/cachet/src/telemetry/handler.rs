// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::Duration;

/// Unique identifier for a cache operation, used to correlate tier events
/// with their parent operation. Generated from a process-wide atomic counter.
pub type RequestId = u64;

/// Data from a per-tier cache operation.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CacheTierEvent<'a> {
    /// Identifies which top-level operation this tier event belongs to.
    pub request_id: RequestId,
    /// Name of the cache tier (for example, "L1" or "L2").
    pub tier_name: &'a str,
    /// Outcome event name (e.g., `attributes::EVENT_HIT`).
    pub outcome: &'a str,
    /// How long the tier operation took.
    pub duration: Duration,
    /// Whether this tier was consulted as a fallback.
    pub fallback: bool,
}

/// Data from a completed top-level cache operation.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CacheOperationEvent<'a> {
    /// Identifies this operation. Matches `request_id` on associated tier events.
    pub request_id: RequestId,
    /// Name of the cache.
    pub cache_name: &'a str,
    /// The operation name (e.g., "cache.get", "cache.insert").
    pub operation: &'a str,
    /// Total duration of the operation.
    pub duration: Duration,
    /// Whether this operation ran with stampede protection enabled.
    pub coalesced: bool,
}

/// Trait for consuming cachet telemetry events.
///
/// Implement this trait to receive structured callbacks for cache operations.
/// Register via [`CacheBuilder::event_handler`](crate::CacheBuilder::event_handler).
///
/// # Example
///
/// ```ignore
/// use cachet::telemetry::handler::{CacheEventHandler, CacheOperationEvent, CacheTierEvent};
///
/// struct MyHandler;
///
/// impl CacheEventHandler for MyHandler {
///     fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
///         println!("tier {} = {} ({}ns)", event.tier_name, event.outcome, event.duration.as_nanos());
///     }
///
///     fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
///         println!("op {} took {}ns", event.operation, event.duration.as_nanos());
///     }
/// }
/// ```
pub trait CacheEventHandler: Send + Sync {
    /// Called for each per-tier sub-operation.
    ///
    /// May be called multiple times per top-level operation (once per tier).
    fn on_tier_event(&self, event: &CacheTierEvent<'_>);

    /// Called once when the top-level cache operation completes.
    fn on_operation_complete(&self, event: &CacheOperationEvent<'_>);
}

/// An owned snapshot of a [`CacheTierEvent`], captured by [`RecordingEventHandler`].
#[cfg(any(feature = "test-util", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedTierEvent {
    /// See [`CacheTierEvent::request_id`].
    pub request_id: RequestId,
    /// See [`CacheTierEvent::tier_name`].
    pub tier_name: String,
    /// See [`CacheTierEvent::outcome`].
    pub outcome: String,
    /// See [`CacheTierEvent::duration`].
    pub duration: Duration,
    /// See [`CacheTierEvent::fallback`].
    pub fallback: bool,
}

/// An owned snapshot of a [`CacheOperationEvent`], captured by [`RecordingEventHandler`].
#[cfg(any(feature = "test-util", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedOperationEvent {
    /// See [`CacheOperationEvent::request_id`].
    pub request_id: RequestId,
    /// See [`CacheOperationEvent::cache_name`].
    pub cache_name: String,
    /// See [`CacheOperationEvent::operation`].
    pub operation: String,
    /// See [`CacheOperationEvent::duration`].
    pub duration: Duration,
    /// See [`CacheOperationEvent::coalesced`].
    pub coalesced: bool,
}

/// A [`CacheEventHandler`] that records events into shared buffers for inspection.
///
/// Available with the `test-util` feature. Handlers are taken by value when
/// registered, so register a *clone* via
/// [`event_handler`](crate::CacheBuilder::event_handler), keep the original, drive the
/// cache, then read the captured events with [`tier_events`](Self::tier_events) and
/// [`operation_events`](Self::operation_events). All clones share the same buffers.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "memory")] {
/// use cachet::{Cache, RecordingEventHandler};
/// use tick::Clock;
/// # futures::executor::block_on(async {
/// let handler = RecordingEventHandler::new();
/// let cache = Cache::builder::<String, i32>(Clock::new_frozen())
///     .memory()
///     .event_handler(handler.clone())
///     .build();
///
/// let _ = cache.get("absent").await;
/// assert!(
///     handler
///         .operation_events()
///         .iter()
///         .any(|event| event.operation == "cache.get")
/// );
/// # });
/// # }
/// ```
#[cfg(any(feature = "test-util", test))]
#[derive(Debug, Clone, Default)]
pub struct RecordingEventHandler {
    tier_events: std::sync::Arc<std::sync::Mutex<Vec<RecordedTierEvent>>>,
    operation_events: std::sync::Arc<std::sync::Mutex<Vec<RecordedOperationEvent>>>,
}

#[cfg(any(feature = "test-util", test))]
impl RecordingEventHandler {
    /// Creates a recording handler with empty buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of the tier events captured so far, in emission order.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned by a thread panicking while holding it.
    #[must_use]
    pub fn tier_events(&self) -> Vec<RecordedTierEvent> {
        self.tier_events
            .lock()
            .expect("recording handler mutex should not be poisoned")
            .clone()
    }

    /// Returns a snapshot of the operation-complete events captured so far, in emission order.
    ///
    /// # Panics
    ///
    /// Panics if the internal lock was poisoned by a thread panicking while holding it.
    #[must_use]
    pub fn operation_events(&self) -> Vec<RecordedOperationEvent> {
        self.operation_events
            .lock()
            .expect("recording handler mutex should not be poisoned")
            .clone()
    }
}

#[cfg(any(feature = "test-util", test))]
impl CacheEventHandler for RecordingEventHandler {
    fn on_tier_event(&self, event: &CacheTierEvent<'_>) {
        self.tier_events
            .lock()
            .expect("recording handler mutex should not be poisoned")
            .push(RecordedTierEvent {
                request_id: event.request_id,
                tier_name: event.tier_name.to_string(),
                outcome: event.outcome.to_string(),
                duration: event.duration,
                fallback: event.fallback,
            });
    }

    fn on_operation_complete(&self, event: &CacheOperationEvent<'_>) {
        self.operation_events
            .lock()
            .expect("recording handler mutex should not be poisoned")
            .push(RecordedOperationEvent {
                request_id: event.request_id,
                cache_name: event.cache_name.to_string(),
                operation: event.operation.to_string(),
                duration: event.duration,
                coalesced: event.coalesced,
            });
    }
}
