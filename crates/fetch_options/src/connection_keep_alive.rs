// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Connection keep-alive configuration.

use std::time::Duration;

use crate::{DEFAULT_KEEP_ALIVE_INTERVAL, DEFAULT_KEEP_ALIVE_TIMEOUT};

/// Controls how HTTP connections are kept alive between requests.
///
/// Keep-alive maintains open connections, reducing latency for subsequent requests
/// by avoiding the overhead of establishing new TCP connections. This can significantly
/// improve performance when making multiple requests to the same server.
///
/// Use [`disabled`](Self::disabled), [`active_connections`](Self::active_connections), or
/// [`active_and_idle_connections`](Self::active_and_idle_connections) to construct values
/// with sensible defaults, or match on the variants directly when full control is required.
#[derive(Debug, Default, Clone)]
pub enum ConnectionKeepAlive {
    /// Keep-alive is disabled.
    ///
    /// Connections aren't actively kept alive. The server may close them at any time,
    /// and they'll be transparently discarded when the client tries to use them again.
    #[default]
    Disabled,
    /// Send keep-alive probes only on connections that are actively handling requests.
    ///
    /// Idle connections are allowed to close naturally.
    ActiveConnections {
        /// How frequently keep-alive probes are sent.
        interval: Duration,
        /// Maximum time to wait for a probe response before closing the connection.
        timeout: Duration,
    },
    /// Send keep-alive probes on all connections, including idle ones sitting in the pool.
    ///
    /// This is the most aggressive connection reuse strategy.
    ActiveAndIdleConnections {
        /// How frequently keep-alive probes are sent.
        interval: Duration,
        /// Maximum time to wait for a probe response before closing the connection.
        timeout: Duration,
    },
}

impl ConnectionKeepAlive {
    /// Disables keep-alive. Equivalent to [`ConnectionKeepAlive::Disabled`].
    #[cfg_attr(test, mutants::skip)] // replacing with default results in the same return value
    #[must_use]
    pub fn disabled() -> Self {
        Self::Disabled
    }

    /// Creates a keep-alive configuration that only maintains active connections.
    ///
    /// This mode sends keep-alive probes for connections currently in use while
    /// allowing idle connections to close naturally. The `interval` controls how
    /// frequently keep-alive probes are sent (defaults to 20 seconds if `None`),
    /// and `timeout` sets the maximum time to wait for a response before closing
    /// the connection (defaults to 20 seconds if `None`).
    #[must_use]
    pub fn active_connections(interval: impl Into<Option<Duration>>, timeout: impl Into<Option<Duration>>) -> Self {
        Self::ActiveConnections {
            interval: interval.into().unwrap_or(DEFAULT_KEEP_ALIVE_INTERVAL),
            timeout: timeout.into().unwrap_or(DEFAULT_KEEP_ALIVE_TIMEOUT),
        }
    }

    /// Creates a keep-alive configuration that maintains both active and idle connections.
    ///
    /// This is the most aggressive connection reuse strategy, keeping all connections alive
    /// whether they're currently in use or waiting in the pool. The `interval` controls how
    /// frequently keep-alive probes are sent (defaults to 20 seconds if `None`), and `timeout`
    /// sets the maximum time to wait for a response before closing the connection (defaults to
    /// 20 seconds if `None`).
    #[must_use]
    pub fn active_and_idle_connections(interval: impl Into<Option<Duration>>, timeout: impl Into<Option<Duration>>) -> Self {
        Self::ActiveAndIdleConnections {
            interval: interval.into().unwrap_or(DEFAULT_KEEP_ALIVE_INTERVAL),
            timeout: timeout.into().unwrap_or(DEFAULT_KEEP_ALIVE_TIMEOUT),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use insta::assert_debug_snapshot;

    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn assert_connection_keep_alive_type() {
        static_assertions::assert_impl_all!(
            ConnectionKeepAlive: Send,
            Sync,
            Clone,
            Debug,
            Default
        );
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn connection_keep_alive_default() {
        assert_debug_snapshot!(ConnectionKeepAlive::default());
    }

    #[test]
    fn connection_keep_alive_disabled() {
        assert!(matches!(ConnectionKeepAlive::disabled(), ConnectionKeepAlive::Disabled));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn connection_keep_alive_active_connections() {
        assert_debug_snapshot!(ConnectionKeepAlive::active_connections(
            Duration::from_secs(10),
            Duration::from_secs(15)
        ));
        assert_debug_snapshot!(ConnectionKeepAlive::active_connections(None, None));
        assert_debug_snapshot!(ConnectionKeepAlive::active_connections(
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(5))
        ));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn connection_keep_alive_active_and_idle_connections() {
        assert_debug_snapshot!(ConnectionKeepAlive::active_and_idle_connections(
            Duration::from_secs(10),
            Duration::from_secs(15)
        ));
        assert_debug_snapshot!(ConnectionKeepAlive::active_and_idle_connections(None, None));
        assert_debug_snapshot!(ConnectionKeepAlive::active_and_idle_connections(
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(5))
        ));
    }
}
