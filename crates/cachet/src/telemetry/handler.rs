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
    /// The span/operation name (e.g., "cache.get", "cache.insert").
    pub operation: &'a str,
    /// Total duration of the operation.
    pub duration: Duration,
    /// Whether the request was coalesced via stampede protection.
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
