// Copyright (c) Microsoft Corporation.

//! The main cache type with telemetry and stampede protection.

use std::{fmt::Debug, hash::Hash, marker::PhantomData};

#[cfg(any(feature = "tokio"))]
use singleflight_async::SingleFlight;
use tick::Clock;

use crate::{Error, builder::CacheBuilder};
use cachelon_tier::{CacheEntry, CacheTier};

/// Type alias for cache names used in telemetry.
pub type CacheName = &'static str;

/// The main cache type providing user-facing API with telemetry.
///
/// `Cache` wraps any `CacheTier` implementation and provides:
/// - Consistent API for all cache operations
/// - Telemetry propagation to inner tiers
/// - Clock management for time-based operations
/// - Optional stampede protection via `get_coalesced`
///
/// This type does NOT implement `CacheTier` - it is always the outermost wrapper.
/// Inner tiers are composed using `CacheWrapper` and `FallbackCache`.
///
/// # Examples
///
/// ## Basic Cache
///
/// ```
/// use cachelon::{Cache, CacheEntry};
/// use tick::Clock;
/// # futures::executor::block_on(async {
///
/// let clock = Clock::new_frozen();
/// let cache = Cache::builder::<String, i32>(clock)
///     .memory()
///     .build();
///
/// cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
/// let value = cache.get(&"key".to_string()).await;
/// assert_eq!(*value.unwrap().value(), 42);
/// # });
/// ```
///
/// ## Multi-Tier Cache
///
/// ```
/// use cachelon::{Cache, FallbackPromotionPolicy};
/// use tick::Clock;
/// use std::time::Duration;
/// # futures::executor::block_on(async {
///
/// let clock = Clock::new_frozen();
/// let l2 = Cache::builder::<String, String>(clock.clone()).memory();
///
/// let cache = Cache::builder::<String, String>(clock)
///     .memory()
///     .ttl(Duration::from_secs(60))
///     .with_fallback(l2)
///     .promotion_policy(FallbackPromotionPolicy::always())
///     .build();
/// # });
/// ```
#[derive(Debug)]
pub struct Cache<K, V, S = ()> {
    pub(crate) name: CacheName,
    pub(crate) storage: S,
    pub(crate) clock: Clock,
    #[cfg(any(feature = "tokio"))]
    coalesce: SingleFlight<K, Option<CacheEntry<V>>>,
    _phantom: PhantomData<(K, V)>,
}

impl Cache<(), (), ()> {
    /// Creates a new cache builder.
    ///
    /// The builder pattern allows configuring storage, TTL, telemetry,
    /// and fallback tiers before constructing the cache.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::Cache;
    /// use tick::Clock;
    /// use std::time::Duration;
    ///
    /// let clock = Clock::new_frozen();
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

/// Constructor and accessor methods.
impl<K, V, S> Cache<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
    S: CacheTier<K, V> + Send + Sync,
{
    pub(crate) fn new(name: CacheName, storage: S, clock: Clock) -> Self {
        Self {
            name,
            storage,
            clock,
            #[cfg(any(feature = "tokio"))]
            coalesce: SingleFlight::new(),
            _phantom: PhantomData,
        }
    }

    /// Returns a reference to the inner storage tier.
    ///
    /// This allows accessing tier-specific functionality not exposed by
    /// the main `Cache` API.
    #[must_use]
    pub fn inner(&self) -> &S {
        &self.storage
    }

    /// Consumes the cache and returns the inner storage tier.
    ///
    /// This is useful when you need to extract the underlying storage
    /// for reuse or inspection.
    #[must_use]
    pub fn into_inner(self) -> S {
        self.storage
    }
}

/// Public API methods - work for both native and mocked caches.
impl<K, V, S> Cache<K, V, S>
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

/// Public API methods that require `CacheTier` dispatch.
impl<K, V, S> Cache<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
    S: CacheTier<K, V> + Send + Sync,
{
    /// Retrieves a value from the cache.
    ///
    /// Returns `None` if the key is not found or the entry has expired.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, CacheEntry};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// let result = cache.get(&"missing".to_string()).await;
    /// assert!(result.is_none());
    /// # });
    /// ```
    pub async fn get(&self, key: &K) -> Option<CacheEntry<V>> {
        self.storage.get(key).await
    }

    /// Retrieves a value from the cache, with error handling.
    ///
    /// Returns `Ok(None)` if the key is not found, or `Err` if the
    /// underlying storage operation fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails.
    pub async fn try_get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        self.storage.try_get(key).await
    }

    /// Inserts a value into the cache.
    ///
    /// The entry's timestamp will be set to the current time according
    /// to the cache's clock.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, CacheEntry};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
    /// # });
    /// ```
    pub async fn insert(&self, key: &K, entry: CacheEntry<V>) {
        self.storage.insert(key, entry).await;
    }

    /// Inserts a value into the cache, with error handling.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails.
    pub async fn try_insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.storage.try_insert(key, entry).await
    }

    /// Invalidates (removes) a value from the cache.
    pub async fn invalidate(&self, key: &K) {
        self.storage.invalidate(key).await;
    }

    /// Invalidates (removes) a value from the cache, returning an error if the operation fails.
    pub async fn try_invalidate(&self, key: &K) -> Result<(), Error> {
        self.storage.try_invalidate(key).await
    }

    /// Returns true if the cache contains a value for the given key.
    pub async fn contains(&self, key: &K) -> bool {
        self.get(key).await.is_some()
    }

    /// Returns true if the cache contains a value for the given key, or an error.
    pub async fn try_contains(&self, key: &K) -> Result<bool, Error> {
        Ok(self.try_get(key).await?.is_some())
    }

    /// Clears all entries from the cache.
    pub async fn clear(&self) {
        self.storage.clear().await;
    }

    /// Clears all entries from the cache, returning an error if the operation fails.
    pub async fn try_clear(&self) -> Result<(), Error> {
        self.storage.try_clear().await
    }

    /// Returns the number of entries in the cache, if supported by the underlying storage.
    #[must_use]
    pub fn len(&self) -> Option<u64> {
        self.storage.len()
    }

    /// Returns true if the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> Option<bool> {
        self.storage.is_empty()
    }

    /// Retrieves a value from cache, or computes and caches it if missing.
    ///
    /// If the key is present, returns the cached value immediately. Otherwise,
    /// calls the provided function to compute the value, inserts it, and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, CacheEntry};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// let entry = cache.get_or_insert(&"key".to_string(), || async { 42 }).await;
    /// assert_eq!(*entry.value(), 42);
    ///
    /// // Second call returns cached value without calling the function
    /// let entry = cache.get_or_insert(&"key".to_string(), || async { 100 }).await;
    /// assert_eq!(*entry.value(), 42);
    /// # });
    /// ```
    pub async fn get_or_insert<Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> CacheEntry<V>
    where
        Fut: Future<Output = V>,
    {
        if let Some(entry) = self.get(key).await {
            return entry;
        }
        let value = f().await;
        let entry = CacheEntry::new(value);
        self.insert(key, entry.clone()).await;
        entry
    }

    /// Retrieves a value from cache, or computes and caches it if missing.
    ///
    /// Like `get_or_insert`, but the provided function can fail. Returns an error
    /// if either the function fails or the cache operation fails.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The provided function returns an error
    /// - The underlying cache tier operation fails
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, CacheEntry, Error};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// let result: std::result::Result<CacheEntry<i32>, Error> = cache
    ///     .try_get_or_insert(&"key".to_string(), || async { Ok(42) })
    ///     .await;
    /// assert!(result.is_ok());
    /// # });
    /// ```
    pub async fn try_get_or_insert<E, Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> Result<CacheEntry<V>, E>
    where
        E: From<Error>,
        Fut: Future<Output = Result<V, E>>,
    {
        if let Some(entry) = self.try_get(key).await? {
            return Ok(entry);
        }
        let value = f().await?;
        let entry = CacheEntry::new(value);
        self.try_insert(key, entry.clone()).await?;
        Ok(entry)
    }

    /// Gets a value from the cache with stampede protection.
    ///
    /// When multiple concurrent requests ask for the same key, only one will
    /// actually perform the cache lookup. The others will wait and receive
    /// a clone of the result.
    ///
    /// This prevents the "thundering herd" problem where many concurrent cache
    /// misses for the same key overwhelm the backend.
    #[cfg(any(feature = "tokio"))]
    pub async fn get_coalesced(&self, key: &K) -> Option<CacheEntry<V>>
    where
        K: 'static,
        V: 'static,
    {
        self.coalesce.work(key.clone(), || async { self.get(key).await }).await
    }
}

/// Service implementation for cache operations.
///
/// This enables `Cache` to participate in service middleware hierarchies,
/// allowing composition with retry, timeout, logging, and other middleware.
#[cfg(feature = "service")]
impl<K, V, S> layered::Service<cachelon_service::CacheOperation<K, V>> for Cache<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync,
{
    type Out = Result<cachelon_service::CacheResponse<V>, Error>;

    async fn execute(&self, input: cachelon_service::CacheOperation<K, V>) -> Self::Out {
        match input {
            cachelon_service::CacheOperation::Get(req) => {
                let entry = self.try_get(&req.key).await?;
                Ok(cachelon_service::CacheResponse::Get(entry))
            }
            cachelon_service::CacheOperation::Insert(req) => {
                self.try_insert(&req.key, req.entry).await?;
                Ok(cachelon_service::CacheResponse::Insert(()))
            }
            cachelon_service::CacheOperation::Invalidate(req) => {
                self.try_invalidate(&req.key).await?;
                Ok(cachelon_service::CacheResponse::Invalidate(()))
            }
            cachelon_service::CacheOperation::Clear => {
                self.try_clear().await?;
                Ok(cachelon_service::CacheResponse::Clear(()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    #[test]
    fn try_get_or_insert_error() {
        block_on(async {
            let clock = Clock::new_frozen();
            let cache = Cache::builder::<String, i32>(clock).memory().build();

            let key = "key".to_string();

            let result: std::result::Result<CacheEntry<i32>, Error> = cache
                .try_get_or_insert(&key, || async { Err(Error::from_message("fetch failed")) })
                .await;

            assert!(result.is_err());
        });
    }
}
