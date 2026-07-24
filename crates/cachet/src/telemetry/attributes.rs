// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public constants for cachet telemetry field names and event values.
//!
//! Use these constants to filter or match cachet telemetry events in a custom
//! `tracing_subscriber::Layer`.
//!
//! **Tier events** (hit, miss, expired, etc.) carry `FIELD_NAME`, `FIELD_EVENT`,
//! and `FIELD_DURATION_NS`. Some events intentionally omit `FIELD_DURATION_NS`
//! to indicate "not timed": `EVENT_INSERT_REJECTED`, `EVENT_EVICTION`, and
//! background `EVENT_EXPIRED` events emitted from eviction listeners.
//!
//! **Operation-complete events** carry `FIELD_NAME`, `FIELD_OPERATION`,
//! `FIELD_DURATION_NS`, and `FIELD_COALESCED`.
//!
//! # Example
//!
//! ```ignore
//! use cachet::telemetry::attributes;
//!
//! // In a tracing Visit impl, match on field values:
//! if event_value == attributes::EVENT_HIT {
//!     // handle cache hit
//! }
//! ```

/// Tracing target prefix for all cachet telemetry events.
///
/// All cachet telemetry events use the module path as the tracing target (e.g.,
/// `cachet::telemetry::cache`), which starts with this prefix. Consumers can
/// filter for all cachet events using prefix matching with `tracing_subscriber`:
/// ```ignore
/// use tracing_subscriber::filter;
/// let filter = filter::Targets::new()
///     .with_target(cachet::telemetry::attributes::TARGET, tracing::Level::DEBUG);
/// ```
pub const TARGET: &str = "cachet";

// -- Field names --

/// Field name for the cache tier name.
pub const FIELD_NAME: &str = "cache.name";

/// Field name for the cache event type.
pub const FIELD_EVENT: &str = "cache.event";

/// Field name for the operation duration in nanoseconds.
pub const FIELD_DURATION_NS: &str = "cache.duration_ns";

/// Field name for the cache operation name.
pub const FIELD_OPERATION: &str = "cache.operation";

/// Field name recording whether stampede protection was enabled for the operation.
pub const FIELD_COALESCED: &str = "cache.coalesced";

// -- Event values (emitted in the `cache.event` field) --

/// Cache entry was found and valid.
pub const EVENT_HIT: &str = "cache.hit";

/// Cache entry was not found.
pub const EVENT_MISS: &str = "cache.miss";

/// Cache entry was found but expired.
pub const EVENT_EXPIRED: &str = "cache.expired";

/// An error occurred during a get operation.
pub const EVENT_GET_ERROR: &str = "cache.get_error";

/// An entry was successfully inserted.
pub const EVENT_INSERTED: &str = "cache.inserted";

/// An insert was rejected by the insert policy.
pub const EVENT_INSERT_REJECTED: &str = "cache.insert_rejected";

/// An error occurred during an insert operation.
pub const EVENT_INSERT_ERROR: &str = "cache.insert_error";

/// A key was successfully invalidated.
pub const EVENT_INVALIDATED: &str = "cache.invalidated";

/// An error occurred during an invalidate operation.
pub const EVENT_INVALIDATE_ERROR: &str = "cache.invalidate_error";

/// Cache was successfully cleared.
pub const EVENT_CLEARED: &str = "cache.cleared";

/// An error occurred during a clear operation.
pub const EVENT_CLEAR_ERROR: &str = "cache.clear_error";

/// A background refresh found data in the fallback tier.
pub const EVENT_REFRESH_HIT: &str = "cache.refresh_hit";

/// A background refresh did not find data in the fallback tier.
pub const EVENT_REFRESH_MISS: &str = "cache.refresh_miss";

/// An entry was evicted/removed due to cache size constraints.
/// Only emitted when eviction telemetry is enabled.
pub const EVENT_EVICTION: &str = "cache.eviction";

/// A stored value failed authentication and could not be recovered, so it was
/// treated as a miss.
///
/// Only emitted when the `encrypt` feature is enabled. Signals a corrupt,
/// truncated, wrong-key, tampered, or relocated value.
pub const EVENT_UNPROTECT_FAILED: &str = "cache.unprotect_failed";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_constants_match_tracing_field_names() {
        // These constants must match the field names used in tracing macros in telemetry/cache.rs.
        assert_eq!(FIELD_NAME, "cache.name");
        assert_eq!(FIELD_EVENT, "cache.event");
        assert_eq!(FIELD_DURATION_NS, "cache.duration_ns");
        assert_eq!(FIELD_OPERATION, "cache.operation");
        assert_eq!(FIELD_COALESCED, "cache.coalesced");
    }

    #[test]
    fn event_constants_are_unique() {
        let events = [
            EVENT_HIT,
            EVENT_MISS,
            EVENT_EXPIRED,
            EVENT_GET_ERROR,
            EVENT_INSERTED,
            EVENT_INSERT_REJECTED,
            EVENT_INSERT_ERROR,
            EVENT_INVALIDATED,
            EVENT_INVALIDATE_ERROR,
            EVENT_CLEARED,
            EVENT_CLEAR_ERROR,
            EVENT_REFRESH_HIT,
            EVENT_REFRESH_MISS,
            EVENT_EVICTION,
            EVENT_UNPROTECT_FAILED,
        ];

        for (i, a) in events.iter().enumerate() {
            for b in &events[i + 1..] {
                assert_ne!(a, b, "duplicate event constant: {a}");
            }
        }
    }
}
