// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The main cache type with telemetry and stampede protection.

use std::borrow::Borrow;
use std::fmt::Debug;
use std::hash::Hash;

use cachet_tier::{CacheEntry, CacheTier, SizeError};
use tick::Clock;
use uniflight::Merger;

use crate::Error;
use crate::builder::CacheBuilder;

/// Type alias for cache names used in telemetry.
///
/// A static reference is used so that names can be embedded in telemetry
/// attributes (metric labels, log fields) without allocating on every
/// cache operation.
pub type CacheName = &'static str;

/// Mergers for stampede protection on all cache operations.
/// Only created when `stampede_protection` is enabled.
struct Mergers<K, V> {
    get: Merger<K, Result<Option<CacheEntry<V>>, Error>>,
    invalidate: Merger<K, Result<(), Error>>,
    get_or_insert: Merger<K, Result<CacheEntry<V>, Error>>,
    try_get_or_insert: Merger<K, Result<CacheEntry<V>, Error>>,
    optionally_get_or_insert: Merger<K, Result<Option<CacheEntry<V>>, Error>>,
}

impl<K, V> Mergers<K, V>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn new() -> Self {
        Self {
            get: Merger::new(),
            invalidate: Merger::new(),
            get_or_insert: Merger::new(),
            try_get_or_insert: Merger::new(),
            optionally_get_or_insert: Merger::new(),
        }
    }
}

impl<K, V> Debug for Mergers<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mergers").finish_non_exhaustive()
    }
}

/// The main cache type providing user-facing API with optional stampede protection.
///
/// `Cache` wraps any `CacheTier` implementation and provides:
/// - Consistent API for basic cache operations (`get`, `insert`, `invalidate`, `clear`)
/// - "Get or compute" operations (`get_or_insert`, `try_get_or_insert`,
///   `optionally_get_or_insert`)
/// - Optional stampede protection for all of the above (enabled via the builder's
///   `.stampede_protection()`)
/// - Clock management for time-based operations
///
/// This type does NOT implement `CacheTier` - it is always the outermost wrapper.
/// Inner tiers are composed using `CacheWrapper` and `FallbackCache`.
///
/// # Examples
///
/// ## Basic Cache
///
/// ```no_run
/// use cachet::{Cache, CacheEntry};
/// use tick::Clock;
/// # async {
///
/// let clock = Clock::new_tokio();
/// let cache = Cache::builder::<String, i32>(clock).memory().build();
///
/// cache.insert("key".to_string(), CacheEntry::new(42)).await?;
/// let value = cache.get("key").await?;
/// assert_eq!(*value.unwrap().value(), 42);
/// # Ok::<(), cachet::Error>(())
/// # };
/// ```
///
/// ## Multi-Tier Cache
///
/// ```no_run
/// use std::time::Duration;
///
/// use cachet::{Cache, FallbackPromotionPolicy};
/// use tick::Clock;
/// # async {
///
/// let clock = Clock::new_tokio();
/// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
///
/// let cache = Cache::builder::<String, String>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .fallback(l2)
///     .promotion_policy(FallbackPromotionPolicy::always())
///     .build();
/// # };
/// ```
#[derive(Debug)]
pub struct Cache<K, V, CT = ()> {
    pub(crate) name: CacheName,
    pub(crate) storage: CT,
    pub(crate) clock: Clock,
    /// Mergers for stampede protection on all operations.
    /// Only present when `stampede_protection` is enabled.
    mergers: Option<Mergers<K, V>>,
}

impl Cache<(), (), ()> {
    /// Creates a new cache builder.
    ///
    /// The builder pattern allows configuring storage, TTL, telemetry,
    /// and fallback tiers before constructing the cache.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use cachet::Cache;
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock)
    ///     .memory()
    ///     .ttl(Duration::from_secs(60))
    ///     .build();
    /// ```
    #[must_use]
    pub fn builder<K, V>(clock: Clock) -> CacheBuilder<K, V> {
        CacheBuilder::new(clock)
    }
}

impl<K, V, CT> Cache<K, V, CT>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync,
{
    pub(crate) fn new(name: CacheName, storage: CT, clock: Clock, stampede_protection: bool) -> Self {
        Self {
            name,
            storage,
            clock,
            mergers: stampede_protection.then(Mergers::new),
        }
    }

    /// Returns a reference to the inner storage tier.
    ///
    /// This allows accessing tier-specific functionality not exposed by
    /// the main `Cache` API.
    #[must_use]
    pub fn inner(&self) -> &CT {
        &self.storage
    }
}

impl<K, V, CT> Cache<K, V, CT>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Returns the name of this cache for telemetry identification.
    #[must_use]
    pub fn name(&self) -> CacheName {
        self.name
    }

    /// Returns a reference to the cache's clock.
    ///
    /// The clock is used for timestamp generation and expiration checks.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        &self.clock
    }
}

impl<K, V, CT> Cache<K, V, CT>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync,
{
    /// Retrieves a value from the cache.
    ///
    /// Returns `None` if the key is not found or the entry has expired.
    ///
    /// # Stampede Protection
    ///
    /// When enabled via [`stampede_protection()`](crate::CacheBuilder::stampede_protection),
    /// concurrent requests for the same key are merged so only one performs the lookup.
    /// All waiters share the result, including errors.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The underlying cache tier operation fails (error is shared with all waiters)
    /// - With stampede protection, if the leader task panics (wrapped as [`uniflight::LeaderPanicked`])
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::{Cache, CacheEntry};
    /// use tick::Clock;
    /// # async {
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// let result = cache.get("missing").await?;
    /// assert!(result.is_none());
    /// # Ok::<(), cachet::Error>(())
    /// # };
    /// ```
    pub async fn get<Q>(&self, key: &Q) -> Result<Option<CacheEntry<V>>, Error>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized + Send + Sync,
    {
        if let Some(mergers) = &self.mergers {
            let owned = key.to_owned();
            let storage = &self.storage;
            mergers
                .get
                .execute(key, move || async move { storage.get(&owned).await })
                .await
                .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
        } else {
            let owned = key.to_owned();
            self.storage.get(&owned).await
        }
    }

    /// Inserts a value into the cache.
    ///
    /// The entry's timestamp will be set to the current time according
    /// to the cache's clock.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::{Cache, CacheEntry};
    /// use tick::Clock;
    /// # async {
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// cache.insert("key".to_string(), CacheEntry::new(42)).await?;
    /// # Ok::<(), cachet::Error>(())
    /// # };
    /// ```
    pub async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.storage.insert(key, entry).await
    }

    /// Invalidates (removes) a value from the cache.
    ///
    /// For multi-tier caches, invalidation is sent to all tiers concurrently.
    ///
    /// # Stampede Protection
    ///
    /// When enabled, concurrent invalidations for the same key are merged.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails. When this
    /// returns an error, it signals that some cache tier failed to remove the
    /// entry. You may wish to retry the call using normal retry semantics in
    /// order to attempt the removal again later.
    pub async fn invalidate<Q>(&self, key: &Q) -> Result<(), Error>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized + Send + Sync,
    {
        if let Some(mergers) = &self.mergers {
            let owned = key.to_owned();
            let storage = &self.storage;
            mergers
                .invalidate
                .execute(key, move || async move { storage.invalidate(&owned).await })
                .await
                .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
        } else {
            let owned = key.to_owned();
            self.storage.invalidate(&owned).await
        }
    }

    /// Returns true if the cache contains a value for the given key.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails.
    pub async fn contains<Q>(&self, key: &Q) -> Result<bool, Error>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized + Send + Sync,
    {
        Ok(self.get(key).await?.is_some())
    }

    /// Clears all entries from the cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails.
    pub async fn clear(&self) -> Result<(), Error> {
        self.storage.clear().await
    }

    /// Returns an **approximate** count of entries, if supported by the underlying storage.
    ///
    /// # Approximation
    ///
    /// The count is approximate for two reasons:
    ///
    /// 1. **Lazy eviction** - entries that have passed their TTL may still be counted
    ///    until the underlying store evicts them. Eviction typically happens lazily
    ///    during cache operations or on a background schedule, so the count can
    ///    temporarily over count after expiry.
    /// 2. **Primary tier only** - for multi-tier (fallback) caches, only the primary
    ///    tier is counted. Entries that exist exclusively in the fallback tier are not
    ///    reflected here.
    ///
    /// Use this value for approximate capacity monitoring and metrics, not for
    /// correctness decisions.
    ///
    /// # Errors
    ///
    /// Returns `Err(SizeError::unsupported())` if the underlying storage does not support size tracking.
    /// Returns an error if the underlying storage tier fails.
    pub async fn len(&self) -> Result<u64, SizeError> {
        self.storage.len().await
    }

    /// Returns Ok(`true`) if the cache appears to contain no entries.
    ///
    /// This is a convenience wrapper around [`len`](Self::len).
    ///
    /// # Errors
    ///
    /// Returns `Err(SizeError::unsupported())` if the underlying storage does not support size tracking.
    /// Returns an error if the underlying storage tier fails.
    pub async fn is_empty(&self) -> Result<bool, SizeError> {
        self.len().await.map(|n| n == 0)
    }

    /// Retrieves a value from cache, or computes and caches it if missing.
    ///
    /// If the key is present, returns the cached value immediately. Otherwise,
    /// calls the provided function to compute the value, inserts it, and returns it.
    ///
    /// # Concurrency
    ///
    /// This method is **not atomic**: there is a gap between the cache lookup and the
    /// subsequent insert (a "time-of-check, time-of-use" or TOCTOU window). During
    /// that window another caller may insert or invalidate the same key. Consequences:
    ///
    /// - **Last writer wins** - if two callers both miss and compute concurrently,
    ///   whichever inserts last determines the cached value. This is standard
    ///   cache-aside behavior and is harmless because caches are ephemeral;
    ///   TTL ensures eventual consistency with the source of truth.
    /// - **Invalidation during compute** - if one caller computes while another
    ///   calls [`invalidate`](Self::invalidate), the computed value may be inserted
    ///   after the invalidation, causing the entry to reappear. The entry will
    ///   still expire naturally if configured.
    ///
    /// Stampede protection (when enabled) **narrows** this window by coalescing
    /// concurrent misses for the same key - only one caller computes while others
    /// share its result. It does not close the window entirely; cross-operation
    /// races (e.g., an `invalidate` arriving mid-compute) are still possible.
    ///
    /// # Stampede Protection
    ///
    /// When enabled via [`stampede_protection()`](crate::CacheBuilder::stampede_protection),
    /// concurrent calls for the same missing key are coalesced - only one caller
    /// computes the value while others wait and share the result.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache operation fails or (with stampede
    /// protection) if the leader task panics.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use tick::Clock;
    /// # async {
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// let entry = cache.get_or_insert("key", || async { 42 }).await?;
    /// assert_eq!(*entry.value(), 42);
    /// # Ok::<(), cachet::Error>(())
    /// # };
    /// ```
    pub async fn get_or_insert<Q, Fut>(&self, key: &Q, f: impl FnOnce() -> Fut + Send) -> Result<CacheEntry<V>, Error>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized + Send + Sync,
        Fut: Future<Output = V> + Send,
    {
        let owned = key.to_owned();
        if let Some(mergers) = &self.mergers {
            mergers
                .get_or_insert
                .execute(key, move || async move { self.do_get_or_insert(&owned, f).await })
                .await
                .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
        } else {
            self.do_get_or_insert(&owned, f).await
        }
    }

    async fn do_get_or_insert<Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> Result<CacheEntry<V>, Error>
    where
        Fut: Future<Output = V>,
    {
        if let Some(entry) = self.storage.get(key).await? {
            return Ok(entry);
        }
        let value = f().await;
        let entry = CacheEntry::new(value);
        self.insert(key.clone(), entry.clone()).await?;
        Ok(entry)
    }

    /// Retrieves a value from cache, or computes and caches it if missing.
    ///
    /// Like [`get_or_insert`](Self::get_or_insert), but the provided function can fail.
    /// Only successful results are cached - errors are not cached, allowing retries.
    ///
    /// # Concurrency
    ///
    /// Subject to the same TOCTOU window as [`get_or_insert`](Self::get_or_insert) -
    /// see its Concurrency section for details.
    ///
    /// # Stampede Protection
    ///
    /// When enabled via [`stampede_protection()`](crate::CacheBuilder::stampede_protection),
    /// concurrent calls for the same missing key are coalesced. If the computation
    /// fails, the error is shared with all waiters but not cached.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The provided function returns an error (wrapped via [`Error::from_source`])
    /// - The underlying cache operation fails
    /// - With stampede protection, if the leader task panics
    ///
    /// Use [`Error::source_as`] to extract the original error type.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::{Cache, Error};
    /// use tick::Clock;
    /// # async {
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// let result = cache
    ///     .try_get_or_insert("key", || async { Ok::<_, std::io::Error>(42) })
    ///     .await;
    /// assert!(result.is_ok());
    /// # };
    /// ```
    pub async fn try_get_or_insert<Q, E, Fut>(&self, key: &Q, f: impl FnOnce() -> Fut + Send) -> Result<CacheEntry<V>, Error>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized + Send + Sync,
        E: std::error::Error + Send + Sync + 'static,
        Fut: Future<Output = Result<V, E>> + Send,
    {
        let owned = key.to_owned();
        if let Some(mergers) = &self.mergers {
            mergers
                .try_get_or_insert
                .execute(key, move || async move { self.do_try_get_or_insert(&owned, f).await })
                .await
                .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
        } else {
            self.do_try_get_or_insert(&owned, f).await
        }
    }

    async fn do_try_get_or_insert<E, Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> Result<CacheEntry<V>, Error>
    where
        E: std::error::Error + Send + Sync + 'static,
        Fut: Future<Output = Result<V, E>>,
    {
        if let Some(entry) = self.storage.get(key).await? {
            return Ok(entry);
        }
        let value = f().await.map_err(Error::from_source)?;
        let entry = CacheEntry::new(value);
        self.insert(key.clone(), entry.clone()).await?;
        Ok(entry)
    }

    /// Retrieves a value from cache, or conditionally computes and caches it.
    ///
    /// Like [`get_or_insert`](Self::get_or_insert), but the function returns `Option<V>`.
    /// Only `Some` values are cached - `None` results are not cached, allowing the
    /// computation to be retried on subsequent calls.
    ///
    /// # Concurrency
    ///
    /// Subject to the same TOCTOU window as [`get_or_insert`](Self::get_or_insert) -
    /// see its Concurrency section for details.
    ///
    /// # Stampede Protection
    ///
    /// When enabled via [`stampede_protection()`](crate::CacheBuilder::stampede_protection),
    /// concurrent calls for the same missing key are coalesced.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache operation fails or (with stampede
    /// protection) if the leader task panics.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::Cache;
    /// use tick::Clock;
    /// # async {
    ///
    /// let clock = Clock::new_tokio();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// // Returns None without caching
    /// let result = cache
    ///     .optionally_get_or_insert("missing", || async { None })
    ///     .await?;
    /// assert!(result.is_none());
    ///
    /// // Returns Some and caches
    /// let result = cache
    ///     .optionally_get_or_insert("key", || async { Some(42) })
    ///     .await?;
    /// assert_eq!(*result.unwrap().value(), 42);
    /// # Ok::<(), cachet::Error>(())
    /// # };
    /// ```
    pub async fn optionally_get_or_insert<Q, Fut>(&self, key: &Q, f: impl FnOnce() -> Fut + Send) -> Result<Option<CacheEntry<V>>, Error>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ToOwned<Owned = K> + ?Sized + Send + Sync,
        Fut: Future<Output = Option<V>> + Send,
    {
        let owned = key.to_owned();
        if let Some(mergers) = &self.mergers {
            mergers
                .optionally_get_or_insert
                .execute(key, move || async move { self.do_optionally_get_or_insert(&owned, f).await })
                .await
                .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
        } else {
            self.do_optionally_get_or_insert(&owned, f).await
        }
    }

    async fn do_optionally_get_or_insert<Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> Result<Option<CacheEntry<V>>, Error>
    where
        Fut: Future<Output = Option<V>>,
    {
        if let Some(entry) = self.storage.get(key).await? {
            return Ok(Some(entry));
        }
        match f().await {
            Some(value) => {
                let entry = CacheEntry::new(value);
                self.insert(key.clone(), entry.clone()).await?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }
}

#[cfg(feature = "service")]
impl<K, V, CT> layered::Service<cachet_service::CacheOperation<K, V>> for Cache<K, V, CT>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync,
{
    type Out = Result<cachet_service::CacheResponse<V>, Error>;

    async fn execute(&self, input: cachet_service::CacheOperation<K, V>) -> Self::Out {
        match input {
            cachet_service::CacheOperation::Get(req) => {
                let entry = self.get(&req.key).await?;
                Ok(cachet_service::CacheResponse::Get(entry))
            }
            cachet_service::CacheOperation::Insert(req) => {
                self.insert(req.key, req.entry).await?;
                Ok(cachet_service::CacheResponse::Insert)
            }
            cachet_service::CacheOperation::Invalidate(req) => {
                self.invalidate(&req.key).await?;
                Ok(cachet_service::CacheResponse::Invalidate)
            }
            cachet_service::CacheOperation::Clear => {
                self.clear().await?;
                Ok(cachet_service::CacheResponse::Clear)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use cachet_tier::MockCache;

    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    fn build_cache() -> Cache<String, i32, crate::wrapper::CacheWrapper<String, i32, MockCache<String, i32>>> {
        let clock = Clock::new_frozen();
        Cache::builder::<String, i32>(clock).storage(MockCache::new()).build()
    }

    fn build_cache_with_stampede() -> Cache<String, i32, crate::wrapper::CacheWrapper<String, i32, MockCache<String, i32>>> {
        let clock = Clock::new_frozen();
        Cache::builder::<String, i32>(clock)
            .storage(MockCache::new())
            .stampede_protection()
            .build()
    }

    #[test]
    fn mergers_new_and_debug() {
        let m = Mergers::<String, i32>::new();
        let debug = format!("{m:?}");
        assert!(debug.contains("Mergers"));
    }

    #[test]
    fn cache_builder_creates_cache() {
        let clock = Clock::new_frozen();
        let _ = Cache::builder::<String, i32>(clock);
    }

    #[test]
    fn cache_new_and_accessors() {
        let cache = build_cache();
        assert!(!cache.name().is_empty());
        let _ = cache.clock();
        let _ = cache.inner();
    }

    #[test]
    fn cache_get_miss() {
        block_on(async {
            let cache = build_cache();
            let result = cache.get("missing").await.unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn cache_insert_and_get() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
            let entry = cache.get("key").await.unwrap().expect("should exist");
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_invalidate_no_stampede() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            cache.invalidate("key").await.unwrap();
            assert!(cache.get("key").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_invalidate_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            cache.invalidate("key").await.unwrap();
            assert!(cache.get("key").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_contains() {
        block_on(async {
            let cache = build_cache();
            assert!(!cache.contains("key").await.unwrap());
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            assert!(cache.contains("key").await.unwrap());
        });
    }

    #[test]
    fn cache_clear() {
        block_on(async {
            let cache = build_cache();
            cache.insert("a".to_string(), CacheEntry::new(1)).await.unwrap();
            cache.clear().await.unwrap();
            assert!(cache.get("a").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_len_and_is_empty() {
        block_on(async {
            let cache = build_cache();
            assert_eq!(cache.len().await.expect("len should return Ok"), 0);
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            assert_eq!(cache.len().await.expect("len should return Ok"), 1);
        });
    }

    #[test]
    fn cache_get_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            cache.insert("key".to_string(), CacheEntry::new(99)).await.unwrap();
            let entry = cache.get("key").await.unwrap().expect("should exist");
            assert_eq!(*entry.value(), 99);
        });
    }

    #[test]
    fn cache_get_miss_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            assert!(cache.get("missing").await.unwrap().is_none());
        });
    }

    #[test]
    fn cache_get_or_insert_miss() {
        block_on(async {
            let cache = build_cache();
            let entry = cache.get_or_insert("key", || async { 42 }).await.unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_get_or_insert_hit() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            let entry = cache.get_or_insert("key", || async { 99 }).await.unwrap();
            assert_eq!(*entry.value(), 1);
        });
    }

    #[test]
    fn cache_get_or_insert_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            let entry = cache.get_or_insert("key", || async { 42 }).await.unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_try_get_or_insert_ok() {
        block_on(async {
            let cache = build_cache();
            let entry = cache
                .try_get_or_insert("key", || async { Ok::<_, std::io::Error>(42) })
                .await
                .unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_try_get_or_insert_hit() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            let entry = cache
                .try_get_or_insert("key", || async { Ok::<_, std::io::Error>(99) })
                .await
                .unwrap();
            assert_eq!(*entry.value(), 1);
        });
    }

    #[test]
    fn cache_try_get_or_insert_err() {
        block_on(async {
            let cache = build_cache();
            let result = cache
                .try_get_or_insert("key", || async { Err::<i32, _>(std::io::Error::other("fail")) })
                .await;
            result.unwrap_err();
        });
    }

    #[test]
    fn cache_try_get_or_insert_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            let entry = cache
                .try_get_or_insert("key", || async { Ok::<_, std::io::Error>(42) })
                .await
                .unwrap();
            assert_eq!(*entry.value(), 42);
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_some() {
        block_on(async {
            let cache = build_cache();
            let entry = cache.optionally_get_or_insert("key", || async { Some(42) }).await.unwrap();
            assert_eq!(*entry.unwrap().value(), 42);
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_none() {
        block_on(async {
            let cache = build_cache();
            let result = cache.optionally_get_or_insert("key", || async { None }).await.unwrap();
            assert!(result.is_none());
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_hit() {
        block_on(async {
            let cache = build_cache();
            cache.insert("key".to_string(), CacheEntry::new(1)).await.unwrap();
            let entry = cache.optionally_get_or_insert("key", || async { Some(99) }).await.unwrap();
            assert_eq!(*entry.unwrap().value(), 1);
        });
    }

    #[test]
    fn cache_optionally_get_or_insert_with_stampede() {
        block_on(async {
            let cache = build_cache_with_stampede();
            let entry = cache.optionally_get_or_insert("key", || async { Some(42) }).await.unwrap();
            assert_eq!(*entry.unwrap().value(), 42);
        });
    }
}
