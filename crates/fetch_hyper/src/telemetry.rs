// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Telemetry helpers and the [`ConnectionInfo`] response extension.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ohno::ErrorLabel;
use opentelemetry::{KeyValue, Value};
use tick::{Clock, Stopwatch};

/// Diagnostic information about the connection that served a response.
///
/// Attached as a response extension by real network connections. Cheap to
/// clone — clones share state.
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    inner: Arc<ConnectionInfoInner>,
}

#[derive(Debug)]
struct ConnectionInfoInner {
    stopwatch: Stopwatch,
    pool_index: usize,
    max_age: Option<Duration>,
    poisoned: AtomicBool,
}

impl ConnectionInfo {
    /// Creates connection metadata tracking elapsed time from now.
    pub(crate) fn new(clock: &Clock, pool_index: usize, max_age: Option<Duration>) -> Self {
        Self {
            inner: Arc::new(ConnectionInfoInner {
                stopwatch: clock.stopwatch(),
                pool_index,
                max_age,
                poisoned: AtomicBool::new(false),
            }),
        }
    }

    /// Time elapsed since the connection was established.
    #[must_use]
    pub fn age(&self) -> Duration {
        self.inner.stopwatch.elapsed()
    }

    /// Pool that served the request (`0` for single-pool clients).
    #[must_use]
    pub fn pool_index(&self) -> usize {
        self.inner.pool_index
    }

    /// `true` once the connection has been marked for removal from the pool.
    #[must_use]
    pub fn poisoned(&self) -> bool {
        // Acquire pairs with the Release in `mark_poisoned`. Observing `true` is
        // sticky, so a relaxed load would also be sound; Acquire makes intent
        // obvious and costs nothing on the targets we support.
        self.inner.poisoned.load(Ordering::Acquire)
    }

    /// Configured cap, or `None` if uncapped.
    #[must_use]
    pub fn max_age(&self) -> Option<Duration> {
        self.inner.max_age
    }

    /// `true` once age has exceeded the configured cap.
    pub(crate) fn is_expired(&self) -> bool {
        self.max_age().is_some_and(|max_age| self.age() > max_age)
    }

    /// Marks the connection as poisoned. Idempotent.
    pub(crate) fn mark_poisoned(&self) {
        // Release: ensure any state updates preceding the poison decision
        // happen-before any reader that observes `true`. Poisoning is monotonic.
        self.inner.poisoned.store(true, Ordering::Release);
    }
}

pub(crate) fn connection_network_protocol_version(connected: &hyper_util::client::legacy::connect::Connected) -> Value {
    if connected.is_negotiated_h2() {
        Value::from("2")
    } else {
        Value::from("1")
    }
}

pub(crate) fn create_connection_attributes(
    uri: &templated_uri::BaseUri,
    connected: &hyper_util::client::legacy::connect::Connected,
) -> Vec<KeyValue> {
    use opentelemetry_semantic_conventions::trace::{NETWORK_PROTOCOL_VERSION, SERVER_ADDRESS, SERVER_PORT, URL_SCHEME};

    vec![
        KeyValue::new(SERVER_ADDRESS, uri.authority().host().to_string()),
        KeyValue::new(SERVER_PORT, server_port_attribute(uri)),
        KeyValue::new(URL_SCHEME, uri.scheme().to_string()),
        KeyValue::new(NETWORK_PROTOCOL_VERSION, connection_network_protocol_version(connected)),
    ]
}

/// Attributes for failed connection attempts.
pub(crate) fn create_connection_failure_attributes(uri: &templated_uri::BaseUri, error_type: ErrorLabel) -> Vec<KeyValue> {
    use opentelemetry_semantic_conventions::trace::{SERVER_ADDRESS, SERVER_PORT, URL_SCHEME};

    vec![
        KeyValue::new(SERVER_ADDRESS, uri.authority().host().to_string()),
        KeyValue::new(SERVER_PORT, server_port_attribute(uri)),
        KeyValue::new(URL_SCHEME, uri.scheme().to_string()),
        KeyValue::new(opentelemetry_semantic_conventions::attribute::ERROR_TYPE, error_type.into_cow()),
    ]
}

/// Returns the server port as an i64 attribute value, using `-1` as a sentinel
/// when no port is known for the URI's scheme. Keeping the attribute present
/// (rather than omitting it) preserves a stable attribute set across all
/// telemetry emissions, which downstream aggregations rely on.
fn server_port_attribute(uri: &templated_uri::BaseUri) -> i64 {
    uri.effective_port().map_or(-1, i64::from)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::error_labels::LABEL_CONNECT;
    use crate::testing::sorted_attributes;

    #[test]
    fn new_records_initial_state() {
        let info = ConnectionInfo::new(&Clock::new_frozen(), 3, Some(Duration::from_secs(60)));
        assert_eq!(info.pool_index(), 3);
        assert_eq!(info.max_age(), Some(Duration::from_secs(60)));
        assert!(!info.poisoned());
    }

    #[test]
    fn mark_poisoned_is_observable_and_idempotent() {
        let info = ConnectionInfo::new(&Clock::new_frozen(), 0, None);
        assert!(!info.poisoned());
        info.mark_poisoned();
        assert!(info.poisoned());
        info.mark_poisoned();
        assert!(info.poisoned());
    }

    #[test]
    fn is_expired_false_without_max_age() {
        let info = ConnectionInfo::new(&Clock::new_frozen(), 0, None);
        assert!(!info.is_expired());
    }

    #[test]
    fn frozen_clock_keeps_age_zero() {
        let info = ConnectionInfo::new(&Clock::new_frozen(), 0, Some(Duration::from_nanos(1)));
        assert_eq!(info.age(), Duration::ZERO);
        // Even with a tiny max_age, no time has elapsed under a frozen clock.
        assert!(!info.is_expired());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn create_connection_attributes_emits_expected_keys() {
        let uri = templated_uri::BaseUri::from_static("https://example.com:8443");
        let connected = hyper_util::client::legacy::connect::Connected::new();
        let attrs = create_connection_attributes(&uri, &connected);
        insta::assert_debug_snapshot!(sorted_attributes(&attrs));
    }

    #[test]
    fn connection_network_protocol_version_h2() {
        let connected = hyper_util::client::legacy::connect::Connected::new().negotiated_h2();
        assert_eq!(connection_network_protocol_version(&connected).as_str(), "2");
    }

    #[test]
    fn connection_network_protocol_version_default_is_1() {
        let connected = hyper_util::client::legacy::connect::Connected::new();
        assert_eq!(connection_network_protocol_version(&connected).as_str(), "1");
    }

    #[test]
    fn is_expired_true_with_clock_control() {
        let control = tick::ClockControl::new();
        let clock = control.to_clock();
        let info = ConnectionInfo::new(&clock, 0, Some(Duration::from_secs(1)));
        assert!(!info.is_expired());
        control.advance(Duration::from_secs(2));
        assert!(info.is_expired());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn create_connection_failure_attributes_includes_error_type() {
        let uri = templated_uri::BaseUri::from_static("https://example.com");
        let attrs = create_connection_failure_attributes(&uri, LABEL_CONNECT);
        insta::assert_debug_snapshot!(sorted_attributes(&attrs));
    }
}
