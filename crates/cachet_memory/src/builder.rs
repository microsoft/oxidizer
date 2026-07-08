// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder for configuring in-memory caches.
//!
//! This module provides a builder API for `InMemoryCache` that abstracts
//! the underlying cache configuration, providing a stable API surface
//! without exposing implementation details.

use std::fmt;
use std::hash::{BuildHasher, Hash};
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use foldhash::fast::RandomState;

use crate::notification::RemovalCause;
use crate::policy::EvictionPolicy;
use crate::tier::InMemoryCache;

/// Type-erased eviction listener.
///
/// Receives the evicted entry's key (as a shared [`Arc`]) and owned value
/// along with the [`RemovalCause`].
pub(crate) type EvictionListener<K, V> = Arc<dyn Fn(Arc<K>, V, RemovalCause) + Send + Sync + 'static>;

/// Type-erased, cause-only removal observer.
///
/// Unlike [`EvictionListener`], observers receive only the [`RemovalCause`] —
/// never the key or value — so registering one never forces a clone of the
/// evicted entry. Used internally by host crates (such as `cachet`) to bridge
/// removals into their own telemetry.
pub(crate) type RemovalObserver = Arc<dyn Fn(RemovalCause) + Send + Sync + 'static>;

/// Builder for configuring an `InMemoryCache`.
///
/// This builder provides a stable API for common cache configuration
/// options without exposing the underlying cache implementation.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// use cachet_memory::InMemoryCache;
///
/// let cache = InMemoryCache::<String, i32>::builder()
///     .max_capacity(1000)
///     .time_to_live(Duration::from_secs(300))
///     .time_to_idle(Duration::from_secs(60))
///     .initial_capacity(100)
///     .name("my-cache")
///     .build();
/// ```
pub struct InMemoryCacheBuilder<K, V, H = RandomState> {
    pub(crate) max_capacity: Option<u64>,
    pub(crate) initial_capacity: Option<usize>,
    pub(crate) time_to_live: Option<Duration>,
    pub(crate) time_to_idle: Option<Duration>,
    pub(crate) name: Option<&'static str>,
    pub(crate) eviction_policy: EvictionPolicy,
    pub(crate) eviction_listener: Option<EvictionListener<K, V>>,
    pub(crate) removal_observers: Vec<RemovalObserver>,
    pub(crate) eviction_telemetry: bool,
    pub(crate) hasher: H,
    _phantom: PhantomData<(K, V)>,
}

impl<K, V, H: fmt::Debug> fmt::Debug for InMemoryCacheBuilder<K, V, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryCacheBuilder")
            .field("max_capacity", &self.max_capacity)
            .field("initial_capacity", &self.initial_capacity)
            .field("time_to_live", &self.time_to_live)
            .field("time_to_idle", &self.time_to_idle)
            .field("name", &self.name)
            .field("eviction_policy", &self.eviction_policy)
            .field("eviction_listener", &self.eviction_listener.as_ref().map(|_| "<set>"))
            .field("removal_observers", &self.removal_observers.len())
            .field("eviction_telemetry", &self.eviction_telemetry)
            .field("hasher", &self.hasher)
            .finish()
    }
}

// `eviction_listener` holds a `dyn Fn`, which is not auto-`UnwindSafe`/`RefUnwindSafe`.
// Assert both explicitly so adding the listener doesn't break downstream code that
// relied on the auto impls. The closure is invoked by moka as a fire-and-forget
// callback; a panic inside it cannot leave observable state in the builder.
impl<K, V, H: UnwindSafe> UnwindSafe for InMemoryCacheBuilder<K, V, H> {}
impl<K, V, H: RefUnwindSafe> RefUnwindSafe for InMemoryCacheBuilder<K, V, H> {}

impl<K, V> Default for InMemoryCacheBuilder<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> InMemoryCacheBuilder<K, V> {
    /// Creates a new builder with default settings.
    ///
    /// The default configuration creates an unbounded cache with `TinyLFU`
    /// eviction policy and no time-based expiration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            max_capacity: None,
            initial_capacity: None,
            time_to_live: None,
            time_to_idle: None,
            name: None,
            eviction_policy: EvictionPolicy::default(),
            eviction_listener: None,
            removal_observers: Vec::new(),
            eviction_telemetry: false,
            hasher: RandomState::default(),
            _phantom: PhantomData,
        }
    }
}

impl<K, V, H> InMemoryCacheBuilder<K, V, H> {
    /// Sets the maximum capacity of the cache.
    ///
    /// Once the capacity is reached, entries will be evicted to make room
    /// for new entries using the `TinyLFU` eviction policy (combination of
    /// LRU eviction and LFU admission).
    ///
    /// If not set, the cache will be unbounded (limited only by available memory).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .max_capacity(10_000)
    ///     .build();
    /// ```
    #[must_use]
    pub fn max_capacity(mut self, capacity: u64) -> Self {
        self.max_capacity = Some(capacity);
        self
    }

    /// Sets the initial capacity (pre-allocation hint) for the cache.
    ///
    /// This can improve performance by avoiding reallocations during
    /// initial population. The cache may still grow beyond this size.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .initial_capacity(100)
    ///     .max_capacity(10_000)
    ///     .build();
    /// ```
    #[must_use]
    pub fn initial_capacity(mut self, capacity: usize) -> Self {
        self.initial_capacity = Some(capacity);
        self
    }

    /// Sets the time-to-live (TTL) for all entries.
    ///
    /// Entries will expire after this duration from insertion, regardless
    /// of access patterns. This is enforced at the cache tier level and is
    /// independent of any per-entry TTL set via `CacheEntry::expires_after()`.
    ///
    /// Expired entries are removed lazily during cache operations and
    /// automatically in the background using hierarchical timer wheels.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .time_to_live(Duration::from_secs(300))
    ///     .build();
    /// ```
    #[must_use]
    pub fn time_to_live(mut self, duration: Duration) -> Self {
        self.time_to_live = Some(duration);
        self
    }

    /// Sets the time-to-idle (TTI) for all entries.
    ///
    /// Entries will expire after this duration of inactivity (no reads or writes).
    /// The timer is reset on each access (get or insert operation).
    ///
    /// Expired entries are removed lazily during cache operations and
    /// automatically in the background using hierarchical timer wheels.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .time_to_idle(Duration::from_secs(60))
    ///     .build();
    /// ```
    #[must_use]
    pub fn time_to_idle(mut self, duration: Duration) -> Self {
        self.time_to_idle = Some(duration);
        self
    }

    /// Sets a name for the cache.
    ///
    /// This name may appear in logs or debugging output from the
    /// underlying cache implementation.
    ///
    /// Requires `&'static str` for consistency with the outer cache builder,
    /// where the name is embedded in every telemetry event. A static reference
    /// avoids cloning on each cache operation. In practice, cache names are
    /// always string literals.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .name("user-cache")
    ///     .build();
    /// ```
    #[must_use]
    pub fn name(mut self, name: &'static str) -> Self {
        self.name = Some(name);
        self
    }

    /// Sets the eviction policy for the cache.
    ///
    /// Controls how entries are chosen for eviction when the cache reaches its
    /// maximum capacity. Defaults to [`EvictionPolicy::tiny_lfu()`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet_memory::InMemoryCacheBuilder;
    /// use cachet_memory::policy::EvictionPolicy;
    ///
    /// let cache = InMemoryCacheBuilder::<String, i32>::new()
    ///     .max_capacity(1000)
    ///     .eviction_policy(EvictionPolicy::lru())
    ///     .build()
    ///     .expect("Failed to build cache");
    /// ```
    #[must_use]
    pub fn eviction_policy(mut self, policy: EvictionPolicy) -> Self {
        self.eviction_policy = policy;
        self
    }

    /// Registers a listener that is called when an entry is removed from the cache.
    ///
    /// The listener receives the evicted entry's key (as a shared
    /// [`Arc`](std::sync::Arc)), its owned value, and a [`RemovalCause`]
    /// indicating why the entry was removed:
    /// `Size` for capacity-driven evictions, `Expired` for TTL/TTI expiration,
    /// `Explicit` for [`invalidate`](cachet_tier::CacheTier::invalidate) or
    /// [`clear`](cachet_tier::CacheTier::clear) calls, and `Replaced` for inserts
    /// that overwrote an existing key.
    ///
    /// The listener runs on the cache's background maintenance task. Keep the
    /// closure cheap; expensive work should be offloaded to a separate task.
    ///
    /// At most one listener is held: calling this more than once replaces the
    /// previously registered listener. To fan out to multiple consumers,
    /// compose them into a single closure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use std::sync::atomic::{AtomicUsize, Ordering};
    ///
    /// use cachet_memory::{InMemoryCache, RemovalCause};
    ///
    /// let evictions = Arc::new(AtomicUsize::new(0));
    /// let counter = Arc::clone(&evictions);
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .max_capacity(100)
    ///     .on_eviction(move |_key, _value, cause| {
    ///         if matches!(cause, RemovalCause::Size | RemovalCause::Expired) {
    ///             counter.fetch_add(1, Ordering::Relaxed);
    ///         }
    ///     })
    ///     .build()
    ///     .expect("Failed to build cache");
    /// ```
    #[must_use]
    pub fn on_eviction<F>(mut self, listener: F) -> Self
    where
        F: Fn(Arc<K>, V, RemovalCause) + Send + Sync + 'static,
        K: 'static,
        V: 'static,
    {
        self.eviction_listener = Some(Arc::new(listener));
        self
    }

    /// Registers an internal, cause-only removal observer.
    ///
    /// Observers receive only the [`RemovalCause`] — never the evicted key or
    /// value — so registering one never forces a clone of the entry. Multiple
    /// observers may be registered; all run on every removal, in registration
    /// order, before the [`on_eviction`](Self::on_eviction) value listener (if any).
    ///
    /// This is not part of the stable public API. It exists so host crates
    /// (such as `cachet`) can bridge removals into their own telemetry without
    /// competing with the user-facing value listener. End users should use
    /// [`on_eviction`](Self::on_eviction) instead.
    #[doc(hidden)]
    #[must_use]
    pub fn with_removal_observer<F>(mut self, observer: F) -> Self
    where
        F: Fn(RemovalCause) + Send + Sync + 'static,
    {
        self.removal_observers.push(Arc::new(observer));
        self
    }

    /// Requests that the host crate install eviction telemetry for this cache.
    ///
    /// This is a marker for `cachet::CacheBuilder::memory_with` to recognize:
    /// when set, the host registers an internal removal observer that emits
    /// `cache.eviction` on capacity evictions ([`RemovalCause::Size`]) and
    /// `cache.expired` on background TTL/TTI expiry ([`RemovalCause::Expired`]).
    /// The observer is independent of any [`on_eviction`](Self::on_eviction)
    /// value listener, so enabling telemetry never clones the evicted value.
    /// [`RemovalCause::Explicit`] and [`RemovalCause::Replaced`] are
    /// intentionally not surfaced, as they are already covered by the host's
    /// `cache.invalidated` and `cache.inserted` events.
    ///
    /// When `InMemoryCache` is constructed directly via [`Self::build`] without
    /// a host, this flag has no effect — use [`Self::on_eviction`] instead.
    #[must_use]
    pub fn with_eviction_telemetry(mut self) -> Self {
        self.eviction_telemetry = true;
        self
    }

    /// Returns whether [`Self::with_eviction_telemetry`] was called on this builder.
    #[must_use]
    pub fn eviction_telemetry_enabled(&self) -> bool {
        self.eviction_telemetry
    }

    /// Sets a custom hash builder for the cache.
    ///
    /// By default, the cache uses [`foldhash::fast::RandomState`] for high-performance
    /// hashing. Use this method to provide an alternative hasher implementation.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::collections::hash_map::RandomState;
    ///
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .with_hasher(RandomState::new())
    ///     .max_capacity(1000)
    ///     .build();
    /// ```
    #[must_use]
    pub fn with_hasher<H2>(self, hasher: H2) -> InMemoryCacheBuilder<K, V, H2> {
        InMemoryCacheBuilder {
            max_capacity: self.max_capacity,
            initial_capacity: self.initial_capacity,
            time_to_live: self.time_to_live,
            time_to_idle: self.time_to_idle,
            name: self.name,
            eviction_policy: self.eviction_policy,
            eviction_listener: self.eviction_listener,
            removal_observers: self.removal_observers,
            eviction_telemetry: self.eviction_telemetry,
            hasher,
            _phantom: PhantomData,
        }
    }

    /// Builds the configured `InMemoryCache`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use cachet_memory::InMemoryCache;
    ///
    /// let cache = InMemoryCache::<String, i32>::builder()
    ///     .max_capacity(1000)
    ///     .time_to_live(Duration::from_secs(300))
    ///     .build();
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid
    /// Configuration is invalid when:
    /// - Initial capacity is greater than max capacity (if max capacity is set)
    /// - Time-to-idle is greater than time-to-live (if both are set)
    pub fn build(self) -> Result<InMemoryCache<K, V, H>, ValidationError>
    where
        K: Hash + Eq + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        H: BuildHasher + Clone + Send + Sync + 'static,
    {
        self.validate()?;
        Ok(InMemoryCache::from_builder(self))
    }

    fn validate(&self) -> Result<(), ValidationError> {
        ValidationError::invalid_capacity(self.max_capacity, self.initial_capacity).map_or(Ok(()), Err)?;
        ValidationError::invalid_time_to(self.time_to_live, self.time_to_idle).map_or(Ok(()), Err)?;
        Ok(())
    }

    pub(crate) fn build_unchecked(self) -> InMemoryCache<K, V, H>
    where
        K: Hash + Eq + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        H: BuildHasher + Clone + Send + Sync + 'static,
    {
        InMemoryCache::from_builder(self)
    }
}

#[ohno::error]
#[display("invalid cache configuration: {reason}")]
pub struct ValidationError {
    reason: String,
}

impl ValidationError {
    fn invalid_capacity(max_capacity: Option<u64>, initial_capacity: Option<usize>) -> Option<Self> {
        let max = max_capacity?;
        let init = initial_capacity?;
        (init as u64 > max).then(|| Self::new(format!("initial_capacity ({init}) exceeds max_capacity ({max})")))
    }

    fn invalid_time_to(time_to_live: Option<Duration>, time_to_idle: Option<Duration>) -> Option<Self> {
        let time_to_idle = time_to_idle?;
        let time_to_live = time_to_live?;
        (time_to_idle > time_to_live)
            .then(|| Self::new(format!("time to idle ({time_to_idle:?}) exceeds time to live ({time_to_live:?}).")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_capacity_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().max_capacity(100);
        assert_eq!(builder.max_capacity, Some(100));
    }

    #[test]
    fn initial_capacity_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().initial_capacity(50);
        assert_eq!(builder.initial_capacity, Some(50));
    }

    #[test]
    fn time_to_live_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_live(Duration::from_mins(5));
        assert_eq!(builder.time_to_live, Some(Duration::from_mins(5)));
    }

    #[test]
    fn time_to_idle_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().time_to_idle(Duration::from_mins(1));
        assert_eq!(builder.time_to_idle, Some(Duration::from_mins(1)));
    }

    #[test]
    fn name_stores_value() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().name("test");
        assert_eq!(builder.name, Some("test"));
    }

    #[test]
    fn eviction_telemetry_defaults_false() {
        let builder = InMemoryCacheBuilder::<String, i32>::new();
        assert!(!builder.eviction_telemetry_enabled());
    }

    #[test]
    fn with_eviction_telemetry_sets_flag() {
        let builder = InMemoryCacheBuilder::<String, i32>::new().with_eviction_telemetry();
        assert!(builder.eviction_telemetry_enabled());
    }

    #[test]
    fn with_hasher_preserves_eviction_telemetry_flag() {
        let builder = InMemoryCacheBuilder::<String, i32>::new()
            .with_eviction_telemetry()
            .with_hasher(std::collections::hash_map::RandomState::new());
        assert!(builder.eviction_telemetry_enabled());
    }

    #[test]
    fn on_eviction_replaces_previous_listener() {
        use std::sync::Mutex;

        let seen: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_first = Arc::clone(&seen);
        let seen_second = Arc::clone(&seen);

        // Register a first listener and invoke it so its body is exercised.
        let builder = InMemoryCacheBuilder::<String, i32>::new().on_eviction(move |key: Arc<String>, value, _cause| {
            assert_eq!((&**key, value), ("k", 42));
            seen_first.lock().unwrap().push("first");
        });
        let first = builder.eviction_listener.clone().expect("first listener should be installed");
        first(Arc::new("k".to_string()), 42, RemovalCause::Size);
        assert_eq!(*seen.lock().unwrap(), vec!["first"]);

        // Registering a second listener replaces the first; only the last runs.
        let builder = builder.on_eviction(move |key: Arc<String>, value, _cause| {
            assert_eq!((&**key, value), ("k", 42));
            seen_second.lock().unwrap().push("second");
        });
        let listener = builder.eviction_listener.expect("listener should be installed");
        listener(Arc::new("k".to_string()), 42, RemovalCause::Size);

        // The replaced first listener did not run again — only "second" was added.
        assert_eq!(*seen.lock().unwrap(), vec!["first", "second"]);
    }

    #[test]
    fn removal_observers_all_run_in_order() {
        use std::sync::Mutex;

        let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let order_first = Arc::clone(&order);
        let order_second = Arc::clone(&order);

        let builder = InMemoryCacheBuilder::<String, i32>::new()
            .with_removal_observer(move |_cause| order_first.lock().unwrap().push("first"))
            .with_removal_observer(move |_cause| order_second.lock().unwrap().push("second"));

        assert_eq!(builder.removal_observers.len(), 2);
        for observer in &builder.removal_observers {
            observer(RemovalCause::Size);
        }
        assert_eq!(*order.lock().unwrap(), vec!["first", "second"]);
    }

    #[test]
    fn debug_impl_renders_all_fields() {
        let builder = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(10)
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_secs(30))
            .name("my_cache")
            .with_eviction_telemetry()
            .with_removal_observer(|_| {})
            .on_eviction(|_, _, _| {});
        let rendered = format!("{builder:?}");
        assert!(rendered.contains("InMemoryCacheBuilder"));
        assert!(rendered.contains("max_capacity: Some(100)"));
        assert!(rendered.contains("initial_capacity: Some(10)"));
        assert!(rendered.contains("time_to_live: Some(60s)"));
        assert!(rendered.contains("time_to_idle: Some(30s)"));
        assert!(rendered.contains("name: Some(\"my_cache\")"));
        assert!(rendered.contains("eviction_telemetry: true"));
        assert!(rendered.contains("removal_observers: 1"));
        assert!(rendered.contains("eviction_listener: Some(\"<set>\")"));
    }

    #[test]
    fn build_max_capacity_lt_initial_capacity_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(101)
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: initial_capacity (101) exceeds max_capacity (100)"
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_max_capacity_eq_initial_capacity_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .max_capacity(100)
            .initial_capacity(100)
            .build();
        result.unwrap();
    }

    #[test]
    fn build_ttl_less_than_tti_returns_validation_error() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_mins(2))
            .build();
        ohno::assert_error_message!(
            result.unwrap_err(),
            "invalid cache configuration: time to idle (120s) exceeds time to live (60s)."
        );
    }

    #[cfg_attr(miri, ignore)] // crossbeam-epoch triggers Stacked Borrows violations under Miri
    #[test]
    fn build_ttl_eq_tti_succeeds() {
        let result = InMemoryCacheBuilder::<String, i32>::new()
            .time_to_live(Duration::from_mins(1))
            .time_to_idle(Duration::from_mins(1))
            .build();
        result.unwrap();
    }

    #[test]
    fn build_eviction_policy_stores_value() {
        let policy = EvictionPolicy::lru();
        let builder = InMemoryCacheBuilder::<String, i32>::new().eviction_policy(policy.clone());
        assert_eq!(builder.eviction_policy, policy);
    }
}
