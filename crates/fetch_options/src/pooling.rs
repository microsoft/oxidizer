// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Connection pooling configuration types.
//!
//! This module contains types for configuring connection pool behavior,
//! including pool lifetime, size limits, and multi-pool distribution strategies.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

// Matches `SocketsHttpHandler.PooledConnectionIdleTimeout` in .NET (1 minute).
const DEFAULT_POOL_LIFETIME: Duration = Duration::from_mins(1);

/// Configuration options for HTTP connection pooling.
///
/// Controls connection pool behavior including connection lifetime and limits.
/// Connection pooling improves performance by reusing established connections
/// rather than creating new ones for each request.
///
/// # Defaults
///
/// | Option | Default |
/// |---|---|
/// | `connection_idle_timeout` | 60 seconds |
/// | `max_connections` | `usize::MAX` (unlimited) |
/// | `multiple_pools` | `None` (single pool) |
/// | `connection_lifetime` | no maximum lifetime |
///
/// # Connection Handling
///
/// The connection pool automatically manages connection lifecycle and reuses established
/// connections to improve performance. However, connections that end without proper
/// graceful shutdown may result in the next request failing.
///
/// When such failures occur, the problematic connection is discarded and subsequent requests
/// will establish a new healthy connection. The pool implements various strategies to minimize
/// the impact of connection failures, but applications should be prepared to handle occasional
/// connection-related errors through appropriate resilience mechanisms.
///
/// ## Server-Side Connection Closure
///
/// A server may decide to close the connection at any time, even when HTTP/2 keep-alive
/// pings are being sent. Whether pings prevent server-side closure depends on the server
/// implementation: some servers treat HTTP/2 pings as active traffic and reset their idle
/// timers, while others ignore pings entirely and close the connection based on their own
/// idle timeout policy.
///
/// For servers that do not treat HTTP/2 pings as active traffic, consider periodically
/// sending real HTTP requests (e.g., lightweight health-check or no-op requests) to force
/// the server to recognize the connection as active and keep it open.
///
/// ## Keep-Alive Pings
///
/// HTTP/2 keep-alive pings can be sent for both active and idle connections. This behavior
/// is controlled by [`ConnectionKeepAlive`](super::ConnectionKeepAlive):
///
/// - [`ConnectionKeepAlive::active_connections`](super::ConnectionKeepAlive::active_connections)
///   sends pings only on connections that are actively handling requests.
/// - [`ConnectionKeepAlive::active_and_idle_connections`](super::ConnectionKeepAlive::active_and_idle_connections)
///   sends pings on all connections, including idle ones sitting in the pool.
///
/// ## Pool Idle Timeout
///
/// Independently of keep-alive pings, the underlying HTTP client (hyper) will close
/// connections that have been idle in the pool for longer than the configured
/// [`connection_idle_timeout`](Self::connection_idle_timeout). The default is 60 seconds. Pass
/// `None` to keep connections in the pool indefinitely, relying solely on the server or
/// keep-alive probes to determine when a connection is closed.
///
/// ## Connection Lifetime
///
/// Independently of idle eviction, [`connection_lifetime`](Self::connection_lifetime) caps
/// the **total** wall-clock age of a pooled connection. After serving a response, a
/// connection older than the configured lifetime is dropped instead of being returned to
/// the pool, forcing the next request to establish a fresh connection. This is useful for
/// picking up `DNS` changes, load-balancer rotations, or refreshed credentials within a
/// bounded window. By default no maximum lifetime is enforced.
///
/// For per-connection customization (e.g. adding jitter so that pools created via
/// [`multiple_pools`](Self::multiple_pools) don't all recycle at the same instant), use
/// [`ConnectionLifetime::per_connection`].
#[derive(Debug, Clone)]
pub struct ConnectionPoolOptions {
    /// How long idle pooled connections are kept before eviction.
    pub connection_idle_timeout: ConnectionIdleTimeout,
    /// Maximum number of idle connections per host.
    pub max_connections: usize,
    /// Optional multi-pool configuration as `(count, selection_strategy)`.
    ///
    /// `None` means a single pool is used.
    pub multiple_pools: Option<(usize, PoolSelection)>,
    /// Maximum wall-clock lifetime policy for pooled connections.
    pub connection_lifetime: ConnectionLifetime,
}

impl Default for ConnectionPoolOptions {
    fn default() -> Self {
        Self {
            connection_idle_timeout: ConnectionIdleTimeout::default(),
            max_connections: usize::MAX,
            multiple_pools: None,
            connection_lifetime: ConnectionLifetime::default(),
        }
    }
}

impl ConnectionPoolOptions {
    /// Caps how long an idle connection stays in the pool before being evicted.
    ///
    /// Pass `None` to disable idle eviction (connections live until the server or a
    /// keep-alive probe closes them). Defaults to 60 seconds.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use fetch_options::ConnectionPoolOptions;
    ///
    /// let options =
    ///     ConnectionPoolOptions::default().connection_idle_timeout(Duration::from_secs(300));
    /// ```
    #[must_use]
    pub fn connection_idle_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.connection_idle_timeout = match timeout.into() {
            Some(duration) => ConnectionIdleTimeout::Limited(duration),
            None => ConnectionIdleTimeout::Unlimited,
        };
        self
    }

    /// Sets the maximum number of idle connections per host in the pool.
    ///
    /// This controls how many idle connections can be kept open for each host.
    /// By default, this value is set to `usize::MAX`, meaning no limit on idle connections.
    #[must_use]
    pub fn max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// Caps the **total** wall-clock lifetime of a pooled connection.
    ///
    /// Unlike [`connection_idle_timeout`](Self::connection_idle_timeout) (which only bounds
    /// idle time), this bounds the time since the connection was established. After serving a
    /// response, a connection older than the configured cap is dropped instead of being
    /// returned to the pool — useful for picking up `DNS` changes, load-balancer rotations, or
    /// refreshed credentials within a bounded window.
    ///
    /// Use [`ConnectionLifetime::fixed`] for a constant cap, [`ConnectionLifetime::per_connection`]
    /// for per-connection customization (e.g. jitter), or [`ConnectionLifetime::unlimited`]
    /// (the default) to disable lifetime-based recycling.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use fetch_options::{ConnectionLifetime, ConnectionPoolOptions};
    ///
    /// let options = ConnectionPoolOptions::default()
    ///     .connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(300)));
    /// ```
    #[must_use]
    pub fn connection_lifetime(mut self, lifetime: ConnectionLifetime) -> Self {
        self.connection_lifetime = lifetime;
        self
    }

    /// Configures multiple connection pools for high-throughput scenarios.
    ///
    /// This creates `count` separate connection pools and distributes requests across them
    /// according to the specified `selection` strategy.
    ///
    /// Passing `count <= 1` disables multi-pool routing (equivalent to `None`).
    ///
    /// # When to Use
    ///
    /// **For most scenarios, multiple pools are not needed.** A single HTTP/2 connection
    /// can handle many concurrent requests efficiently through multiplexing.
    ///
    /// Only enable multiple pools if you have measured and confirmed that your client is being
    /// throttled by a single HTTP/2 connection. This can happen in very high-throughput
    /// scenarios where the connection's stream limit becomes a bottleneck.
    #[must_use]
    pub fn multiple_pools(mut self, count: usize, selection: PoolSelection) -> Self {
        self.multiple_pools = (count > 1).then_some((count, selection));
        self
    }
}

/// Backs [`ConnectionPoolOptions::connection_idle_timeout`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ConnectionIdleTimeout {
    /// Disable idle-timeout eviction.
    Unlimited,
    /// Evict pooled connections that have been idle longer than the duration.
    Limited(Duration),
}

impl Default for ConnectionIdleTimeout {
    fn default() -> Self {
        Self::Limited(DEFAULT_POOL_LIFETIME)
    }
}

/// Backs [`ConnectionPoolOptions::connection_lifetime`].
///
/// Construct values via [`ConnectionLifetime::unlimited`], [`ConnectionLifetime::fixed`],
/// or [`ConnectionLifetime::per_connection`].
#[derive(Clone, Default)]
#[repr(transparent)]
pub struct ConnectionLifetime(Inner);

#[derive(Clone, Default)]
enum Inner {
    #[default]
    Unlimited,
    Fixed(Duration),
    PerConnection(Arc<dyn Fn() -> Option<Duration> + Send + Sync + 'static>),
}

impl ConnectionLifetime {
    /// Returns a policy that disables lifetime-based recycling.
    ///
    /// This is the default.
    #[must_use]
    pub const fn unlimited() -> Self {
        Self(Inner::Unlimited)
    }

    /// Returns a policy that caps every connection at the given wall-clock age.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use fetch_options::ConnectionLifetime;
    ///
    /// let policy = ConnectionLifetime::fixed(Duration::from_secs(300));
    /// ```
    #[must_use]
    pub const fn fixed(duration: Duration) -> Self {
        Self(Inner::Fixed(duration))
    }

    /// Returns a policy that evaluates `generator` once per new connection to
    /// determine that connection's cap (`None` opts it out of recycling).
    ///
    /// Useful for jitter caps across connections so that pools created via
    /// [`ConnectionPoolOptions::multiple_pools`] don't all recycle at the same instant.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use fetch_options::ConnectionLifetime;
    ///
    /// let counter = std::sync::atomic::AtomicU64::new(0);
    /// let policy = ConnectionLifetime::per_connection(move || {
    ///     let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ///     Some(Duration::from_secs(300 + n % 300))
    /// });
    /// # let _ = policy;
    /// ```
    #[must_use]
    pub fn per_connection<F>(generator: F) -> Self
    where
        F: Fn() -> Option<Duration> + Send + Sync + 'static,
    {
        Self(Inner::PerConnection(Arc::new(generator)))
    }

    /// Resolves the cap for a freshly established connection.
    ///
    /// Returns `None` when no lifetime cap should be applied, or `Some(duration)`
    /// with the maximum age for the connection. For [`ConnectionLifetime::per_connection`]
    /// policies this invokes the user-supplied closure exactly once per call.
    #[must_use]
    pub fn resolve(&self) -> Option<Duration> {
        match &self.0 {
            Inner::Unlimited => None,
            Inner::Fixed(duration) => Some(*duration),
            Inner::PerConnection(generator) => generator(),
        }
    }
}

impl fmt::Debug for ConnectionLifetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Inner::Unlimited => f.debug_tuple("ConnectionLifetime").field(&format_args!("Unlimited")).finish(),
            Inner::Fixed(duration) => f.debug_tuple("ConnectionLifetime").field(duration).finish(),
            Inner::PerConnection(_) => f.debug_tuple("ConnectionLifetime").field(&format_args!("<closure>")).finish(),
        }
    }
}

/// Selects a specific connection pool for a request by index.
///
/// When inserted into a request's [extensions](http::Extensions), this directs the
/// request to the pool at the given index. If the index is out of bounds or the
/// client uses a single pool, the default pool selection strategy is used instead.
///
/// # Example
///
/// ```rust
/// use fetch_options::PoolIndex;
/// use http::Request;
///
/// let mut request = Request::get("https://example.com").body(()).unwrap();
///
/// request.extensions_mut().insert(PoolIndex::new(2));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolIndex(usize);

impl PoolIndex {
    /// Creates a new `PoolIndex` targeting the pool at the given zero-based index.
    #[must_use]
    pub fn new(index: usize) -> Self {
        Self(index)
    }

    /// Returns the pool index.
    #[must_use]
    pub fn index(self) -> usize {
        self.0
    }
}

impl fmt::Display for PoolIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PoolIndex({})", self.0)
    }
}

/// Configures how requests are distributed across a pool of connections.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PoolSelection {
    mode: Mode,
}

impl PoolSelection {
    /// Default number of requests routed to a single connection by
    /// [`PoolSelection::saturating`] before moving to the next.
    pub const DEFAULT_REQUESTS_PER_CLIENT: u32 = 100;

    /// Creates a `PoolSelection` that routes requests in a round-robin fashion,
    /// with each connection handling a well-defined number of requests before
    /// moving to the next connection.
    ///
    /// Use [`PoolSelection::DEFAULT_REQUESTS_PER_CLIENT`] for the standard
    /// threshold (100 requests), or pass any other `u32` value.
    ///
    /// # Panics
    ///
    /// Panics if `requests_per_client` is `0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::PoolSelection;
    ///
    /// // With the default threshold
    /// let selection = PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT);
    ///
    /// // With a custom threshold
    /// let selection = PoolSelection::saturating(50);
    /// ```
    #[must_use]
    pub fn saturating(requests_per_client: u32) -> Self {
        assert!(requests_per_client > 0, "requests_per_client must be > 0");

        Self {
            mode: Mode::Saturating { requests_per_client },
        }
    }

    /// Creates a `PoolSelection` that distributes requests evenly across
    /// connections in a strict round-robin fashion.
    ///
    /// Each successive request is routed to the next connection in the pool,
    /// cycling back to the first connection after reaching the last one.
    /// This provides the most even distribution of requests across all pools.
    #[must_use]
    pub fn round_robin() -> Self {
        Self { mode: Mode::RoundRobin }
    }

    /// Converts this selection strategy into a selector function.
    ///
    /// Returns a function that can be called repeatedly to select clients from an array.
    /// The function maintains internal state to track round-robin or saturating distribution.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch_options::PoolSelection;
    ///
    /// let selector = PoolSelection::round_robin().into_selector::<i32>();
    /// let clients = [1, 2, 3];
    ///
    /// let (first, index) = selector(&clients);
    /// assert_eq!(*first, 1);
    /// assert_eq!(index.index(), 0);
    /// ```
    pub fn into_selector<T>(self) -> impl Fn(&[T]) -> (&T, PoolIndex) {
        let strategy = PoolSelectionStrategy::from(self);
        move |clients: &[T]| strategy.select(clients)
    }
}

#[derive(Debug, Clone)]
enum Mode {
    Saturating { requests_per_client: u32 },
    RoundRobin,
}

#[derive(Debug)]
pub(crate) enum PoolSelectionStrategy {
    Saturating { requests_per_client: u32, counter: AtomicU32 },
    RoundRobin { counter: AtomicU32 },
}

impl PoolSelectionStrategy {
    pub(crate) fn select<'a, T>(&self, clients: &'a [T]) -> (&'a T, PoolIndex) {
        assert!(!clients.is_empty(), "clients must not be empty");
        match self {
            Self::Saturating {
                requests_per_client,
                counter,
            } => {
                let counter = counter.fetch_add(1, Ordering::Relaxed);
                let i = (counter / requests_per_client) as usize % clients.len();

                (&clients[i], PoolIndex::new(i))
            }
            Self::RoundRobin { counter } => {
                let counter = counter.fetch_add(1, Ordering::Relaxed);
                let i = counter as usize % clients.len();

                (&clients[i], PoolIndex::new(i))
            }
        }
    }
}

impl From<PoolSelection> for PoolSelectionStrategy {
    fn from(mode: PoolSelection) -> Self {
        match mode.mode {
            Mode::Saturating { requests_per_client } => Self::Saturating {
                requests_per_client,
                counter: AtomicU32::new(0),
            },
            Mode::RoundRobin => Self::RoundRobin {
                counter: AtomicU32::new(0),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::*;

    #[test]
    fn connection_idle_timeout_default() {
        let default = ConnectionIdleTimeout::default();
        assert!(matches!(
            default,
            ConnectionIdleTimeout::Limited(d) if d == DEFAULT_POOL_LIFETIME
        ));
    }

    #[test]
    fn connection_idle_timeout_debug() {
        assert_eq!(format!("{:?}", ConnectionIdleTimeout::Unlimited), "Unlimited");
        assert_eq!(
            format!("{:?}", ConnectionIdleTimeout::Limited(Duration::from_secs(1))),
            "Limited(1s)"
        );
    }

    #[test]
    fn assert_connection_idle_timeout_type() {
        static_assertions::assert_impl_all!(
            ConnectionIdleTimeout: Send,
            Sync,
            Clone,
            Debug,
            Default
        );
    }

    #[test]
    fn connection_pool_options_default() {
        let options = ConnectionPoolOptions::default();
        assert_eq!(options.max_connections, usize::MAX);
        assert!(matches!(
            options.connection_idle_timeout,
            ConnectionIdleTimeout::Limited(d) if d == Duration::from_secs(60)
        ));
    }

    #[test]
    fn connection_pool_options_connection_idle_timeout_set() {
        let options = ConnectionPoolOptions::default().connection_idle_timeout(Duration::from_secs(120));
        assert!(matches!(
            options.connection_idle_timeout,
            ConnectionIdleTimeout::Limited(d) if d == Duration::from_secs(120)
        ));
    }

    #[test]
    fn connection_pool_options_connection_idle_timeout_none() {
        let options = ConnectionPoolOptions::default()
            .connection_idle_timeout(Duration::from_mins(1))
            .connection_idle_timeout(None);
        assert!(matches!(options.connection_idle_timeout, ConnectionIdleTimeout::Unlimited));
    }

    #[test]
    fn connection_pool_options_max_connections() {
        let options = ConnectionPoolOptions::default().max_connections(100);
        assert_eq!(options.max_connections, 100);
    }

    #[test]
    fn connection_idle_timeout_field_returns_configured_value() {
        let options = ConnectionPoolOptions::default().connection_idle_timeout(Duration::from_secs(45));
        assert!(matches!(
            options.connection_idle_timeout,
            ConnectionIdleTimeout::Limited(d) if d == Duration::from_secs(45)
        ));
    }

    #[test]
    fn connection_idle_timeout_field_returns_unlimited_when_disabled() {
        let options = ConnectionPoolOptions::default().connection_idle_timeout(None);
        assert!(matches!(options.connection_idle_timeout, ConnectionIdleTimeout::Unlimited));
    }

    #[test]
    fn connection_idle_timeout_field_default_is_sixty_seconds() {
        let options = ConnectionPoolOptions::default();
        assert!(matches!(
            options.connection_idle_timeout,
            ConnectionIdleTimeout::Limited(d) if d == Duration::from_secs(60)
        ));
    }

    #[test]
    fn max_connections_field_returns_configured_value() {
        let options = ConnectionPoolOptions::default().max_connections(42);
        assert_eq!(options.max_connections, 42);
    }

    #[test]
    fn max_connections_field_default_is_unlimited() {
        let options = ConnectionPoolOptions::default();
        assert_eq!(options.max_connections, usize::MAX);
    }

    #[test]
    fn connection_lifetime_field_returns_configured_value() {
        let options = ConnectionPoolOptions::default().connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(3600)));
        assert_eq!(options.connection_lifetime.resolve(), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn connection_lifetime_field_default_is_unlimited() {
        let options = ConnectionPoolOptions::default();
        assert_eq!(options.connection_lifetime.resolve(), None);
    }

    #[test]
    fn connection_lifetime_field_returns_per_connection() {
        let options =
            ConnectionPoolOptions::default().connection_lifetime(ConnectionLifetime::per_connection(|| Some(Duration::from_secs(120))));
        assert_eq!(options.connection_lifetime.resolve(), Some(Duration::from_secs(120)));
    }

    #[test]
    fn connection_pool_options_connection_lifetime_default() {
        let options = ConnectionPoolOptions::default();
        assert_eq!(options.connection_lifetime.resolve(), None);
    }

    #[test]
    fn connection_pool_options_connection_lifetime_set() {
        let options = ConnectionPoolOptions::default().connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(3600)));
        assert_eq!(options.connection_lifetime.resolve(), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn connection_pool_options_connection_lifetime_set_unlimited() {
        let options = ConnectionPoolOptions::default()
            .connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(60)))
            .connection_lifetime(ConnectionLifetime::unlimited());
        assert_eq!(options.connection_lifetime.resolve(), None);
    }

    #[test]
    fn connection_pool_options_connection_lifetime_per_connection() {
        let options =
            ConnectionPoolOptions::default().connection_lifetime(ConnectionLifetime::per_connection(|| Some(Duration::from_secs(7))));
        assert_eq!(options.connection_lifetime.resolve(), Some(Duration::from_secs(7)));
    }

    #[test]
    fn connection_lifetime_resolve_unlimited() {
        assert_eq!(ConnectionLifetime::unlimited().resolve(), None);
    }

    #[test]
    fn connection_lifetime_resolve_fixed() {
        let policy = ConnectionLifetime::fixed(Duration::from_secs(10));
        assert_eq!(policy.resolve(), Some(Duration::from_secs(10)));
    }

    #[test]
    fn connection_lifetime_resolve_per_connection_evaluates_closure() {
        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let policy = ConnectionLifetime::per_connection(move || {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(Duration::from_secs(42))
        });

        assert_eq!(policy.resolve(), Some(Duration::from_secs(42)));
        assert_eq!(policy.resolve(), Some(Duration::from_secs(42)));
        assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn connection_lifetime_resolve_per_connection_can_return_none() {
        let policy = ConnectionLifetime::per_connection(|| None);
        assert_eq!(policy.resolve(), None);
    }

    #[test]
    fn connection_lifetime_debug() {
        assert!(format!("{:?}", ConnectionLifetime::unlimited()).contains("Unlimited"));
        let fixed = format!("{:?}", ConnectionLifetime::fixed(Duration::from_secs(1)));
        assert!(fixed.contains("1s"));
        let policy = ConnectionLifetime::per_connection(|| None);
        assert!(format!("{policy:?}").contains("<closure>"));
    }

    #[test]
    fn saturating_ok() {
        let mode = PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT);

        assert!(matches!(mode.mode, Mode::Saturating { requests_per_client: 100 }));
    }

    #[test]
    fn saturating_with_custom_value() {
        let mode = PoolSelection::saturating(50);

        assert!(matches!(mode.mode, Mode::Saturating { requests_per_client: 50 }));
    }

    #[test]
    #[should_panic(expected = "requests_per_client must be > 0")]
    fn saturating_panics_on_zero() {
        let _ = PoolSelection::saturating(0);
    }

    #[test]
    fn default_requests_per_client_constant() {
        assert_eq!(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT, 100);
    }

    #[test]
    fn distributes_requests_across_clients() {
        let clients = [1, 2];
        let strategy = PoolSelectionStrategy::from(PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT));

        // saturate first
        for _ in 0..100 {
            assert_eq!(strategy.select(&clients).0, &1);
        }

        // saturate second
        for _ in 0..100 {
            assert_eq!(strategy.select(&clients).0, &2);
        }

        // back to beginning
        for _ in 0..100 {
            assert_eq!(strategy.select(&clients).0, &1);
        }
    }

    #[test]
    fn round_robin_ok() {
        let mode = PoolSelection::round_robin();

        assert!(matches!(mode.mode, Mode::RoundRobin));
    }

    #[test]
    fn round_robin_distributes_requests_evenly() {
        let clients = [1, 2, 3];
        let strategy = PoolSelectionStrategy::from(PoolSelection::round_robin());

        // Each request should go to the next client in sequence
        let selection = strategy.select(&clients);
        assert_eq!(selection.0, &1);
        assert_eq!(selection.1, PoolIndex::new(0));

        assert_eq!(strategy.select(&clients).0, &2);
        assert_eq!(strategy.select(&clients).0, &3);

        // Wraps back around
        assert_eq!(strategy.select(&clients).0, &1);
        assert_eq!(strategy.select(&clients).0, &2);
        assert_eq!(strategy.select(&clients).0, &3);
    }

    #[test]
    fn round_robin_with_two_clients() {
        let clients = [1, 2];
        let strategy = PoolSelectionStrategy::from(PoolSelection::round_robin());

        for _ in 0..50 {
            assert_eq!(strategy.select(&clients).0, &1);
            assert_eq!(strategy.select(&clients).0, &2);
        }
    }

    #[test]
    fn multiple_pools_field_returns_none_when_single_pool() {
        let options = ConnectionPoolOptions::default();
        assert!(options.multiple_pools.is_none());
    }

    #[test]
    fn multiple_pools_field_returns_some_when_configured() {
        let selection = PoolSelection::saturating(50);
        let options = ConnectionPoolOptions::default().multiple_pools(4, selection);

        let result = options.multiple_pools;
        assert!(result.is_some());

        let (count, _sel) = result.unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn multiple_pools_field_returns_none_when_pool_count_is_one() {
        let selection = PoolSelection::round_robin();
        let options = ConnectionPoolOptions::default().multiple_pools(1, selection);

        assert!(options.multiple_pools.is_none());
    }

    #[test]
    fn multiple_pools_field_with_round_robin() {
        let selection = PoolSelection::round_robin();
        let options = ConnectionPoolOptions::default().multiple_pools(3, selection);

        let result = options.multiple_pools;
        assert_eq!(result.map(|(count, _)| count), Some(3));
    }

    #[test]
    fn into_selector_round_robin_with_integers() {
        let selector = PoolSelection::round_robin().into_selector::<i32>();
        let clients = [10, 20, 30];

        // First call should return first client
        let (first, index) = selector(&clients);
        assert_eq!(*first, 10);
        assert_eq!(index, PoolIndex::new(0));

        // Second call should return second client
        let (second, index) = selector(&clients);
        assert_eq!(*second, 20);
        assert_eq!(index, PoolIndex::new(1));

        // Third call should return third client
        let (third, index) = selector(&clients);
        assert_eq!(*third, 30);
        assert_eq!(index, PoolIndex::new(2));

        // Fourth call should wrap around to first client
        let (wrapped, index) = selector(&clients);
        assert_eq!(*wrapped, 10);
        assert_eq!(index, PoolIndex::new(0));
    }

    #[test]
    fn into_selector_saturating_with_strings() {
        let selector = PoolSelection::saturating(2).into_selector::<&str>();
        let clients = ["a", "b"];

        // First 2 calls should return first client
        assert_eq!(*selector(&clients).0, "a");
        assert_eq!(*selector(&clients).0, "a");

        // Next 2 calls should return second client
        assert_eq!(*selector(&clients).0, "b");
        assert_eq!(*selector(&clients).0, "b");

        // Then back to first client
        assert_eq!(*selector(&clients).0, "a");
    }

    #[test]
    fn multiple_pools_field_returns_owned_selection() {
        let selection = PoolSelection::round_robin();
        let options = ConnectionPoolOptions::default().multiple_pools(2, selection);

        let (count, owned_selection) = options.multiple_pools.unwrap();
        assert_eq!(count, 2);

        // We should be able to use the owned selection to create a selector
        let selector = owned_selection.into_selector::<i32>();
        let clients = [1, 2];
        let (selected, _) = selector(&clients);
        assert_eq!(*selected, 1);
    }

    #[test]
    fn pool_index_index_returns_value() {
        assert_eq!(PoolIndex::new(7).index(), 7);
    }

    #[test]
    fn pool_index_display() {
        assert_eq!(PoolIndex::new(3).to_string(), "PoolIndex(3)");
    }
}
