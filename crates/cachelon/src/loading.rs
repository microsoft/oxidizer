// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Loading cache that wraps a cache and provides get-or-compute operations.

use std::fmt::Debug;
use std::hash::Hash;

use uniflight::Merger;

use crate::{Cache, Error};
use cachelon_tier::{CacheEntry, CacheTier};

/// Mergers for stampede protection on loader operations.
struct LoaderMergers<K, V> {
    get_or_insert: Merger<K, Result<CacheEntry<V>, Error>>,
    try_get_or_insert: Merger<K, Result<CacheEntry<V>, Error>>,
    optionally_get_or_insert: Merger<K, Result<Option<CacheEntry<V>>, Error>>,
}

impl<K, V> LoaderMergers<K, V>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn new() -> Self {
        Self {
            get_or_insert: Merger::new(),
            try_get_or_insert: Merger::new(),
            optionally_get_or_insert: Merger::new(),
        }
    }
}

impl<K, V> Debug for LoaderMergers<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoaderMergers").finish_non_exhaustive()
    }
}

/// A cache wrapper that provides "get or compute" operations with stampede protection.
///
/// `LoadingCache` wraps a [`Cache`] and adds methods for computing and caching values
/// on demand. All loader operations have built-in stampede protection - concurrent
/// requests for the same missing key are coalesced so only one computes the value.
///
/// # Examples
///
/// ```
/// use cachelon::{Cache, LoadingCache, CacheEntry};
/// use tick::Clock;
/// # futures::executor::block_on(async {
///
/// let clock = Clock::new_frozen();
/// let cache = Cache::builder::<String, i32>(clock).memory().build();
/// let loader = LoadingCache::new(cache);
///
/// // First call computes and caches the value
/// let entry = loader.get_or_insert(&"key".to_string(), || async { 42 }).await?;
/// assert_eq!(*entry.value(), 42);
///
/// // Second call returns cached value without calling the function
/// let entry = loader.get_or_insert(&"key".to_string(), || async { 100 }).await?;
/// assert_eq!(*entry.value(), 42);
/// # Ok::<(), cachelon::Error>(())
/// # });
/// ```
#[derive(Debug)]
pub struct LoadingCache<K, V, S> {
    cache: Cache<K, V, S>,
    mergers: LoaderMergers<K, V>,
}

impl<K, V, S> LoadingCache<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync,
{
    /// Creates a new loading cache wrapping the given cache.
    pub fn new(cache: Cache<K, V, S>) -> Self {
        Self {
            cache,
            mergers: LoaderMergers::new(),
        }
    }

    /// Returns a reference to the underlying cache.
    #[must_use]
    pub fn cache(&self) -> &Cache<K, V, S> {
        &self.cache
    }

    /// Consumes the loading cache and returns the underlying cache.
    #[must_use]
    pub fn into_cache(self) -> Cache<K, V, S> {
        self.cache
    }

    /// Retrieves a value from cache, or computes and caches it if missing.
    ///
    /// If the key is present, returns the cached value immediately. Otherwise,
    /// calls the provided function to compute the value, inserts it, and returns it.
    ///
    /// # Stampede Protection
    ///
    /// Concurrent calls for the same missing key are coalesced. Only one caller
    /// computes the value while others wait and share the result.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache operation fails or if the leader
    /// task panics.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, LoadingCache};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    /// let loader = LoadingCache::new(cache);
    ///
    /// let entry = loader.get_or_insert(&"key".to_string(), || async { 42 }).await?;
    /// assert_eq!(*entry.value(), 42);
    /// # Ok::<(), cachelon::Error>(())
    /// # });
    /// ```
    pub async fn get_or_insert<Fut>(&self, key: &K, f: impl FnOnce() -> Fut + Send) -> Result<CacheEntry<V>, Error>
    where
        Fut: Future<Output = V> + Send,
    {
        self.mergers
            .get_or_insert
            .execute(key, || Box::pin(self.do_get_or_insert(key, f)))
            .await
            .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
    }

    async fn do_get_or_insert<Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> Result<CacheEntry<V>, Error>
    where
        Fut: Future<Output = V>,
    {
        if let Some(entry) = self.cache.get(key).await? {
            return Ok(entry);
        }
        let value = f().await;
        let entry = CacheEntry::new(value);
        self.cache.insert(key, entry.clone()).await?;
        Ok(entry)
    }

    /// Retrieves a value from cache, or computes and caches it if missing.
    ///
    /// Like [`get_or_insert`](Self::get_or_insert), but the provided function can fail.
    /// Only successful results are cached - errors are not cached, allowing retries.
    ///
    /// # Stampede Protection
    ///
    /// Concurrent calls for the same missing key are coalesced. Only one caller
    /// computes the value while others wait and share the result. If the computation
    /// fails, the error is shared with all waiters but not cached.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The provided function returns an error (wrapped via [`Error::from_source`])
    /// - The underlying cache operation fails
    /// - The leader task panics
    ///
    /// Use [`Error::source_as`] to extract the original error type.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, LoadingCache, Error};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    /// let loader = LoadingCache::new(cache);
    ///
    /// let result = loader
    ///     .try_get_or_insert(&"key".to_string(), || async { Ok::<_, std::io::Error>(42) })
    ///     .await;
    /// assert!(result.is_ok());
    /// # });
    /// ```
    pub async fn try_get_or_insert<E, Fut>(&self, key: &K, f: impl FnOnce() -> Fut + Send) -> Result<CacheEntry<V>, Error>
    where
        E: std::error::Error + Send + Sync + 'static,
        Fut: Future<Output = Result<V, E>> + Send,
    {
        self.mergers
            .try_get_or_insert
            .execute(key, || Box::pin(self.do_try_get_or_insert(key, f)))
            .await
            .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
    }

    async fn do_try_get_or_insert<E, Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> Result<CacheEntry<V>, Error>
    where
        E: std::error::Error + Send + Sync + 'static,
        Fut: Future<Output = Result<V, E>>,
    {
        if let Some(entry) = self.cache.get(key).await? {
            return Ok(entry);
        }
        let value = f().await.map_err(Error::from_source)?;
        let entry = CacheEntry::new(value);
        self.cache.insert(key, entry.clone()).await?;
        Ok(entry)
    }

    /// Retrieves a value from cache, or conditionally computes and caches it.
    ///
    /// Like [`get_or_insert`](Self::get_or_insert), but the function returns `Option<V>`.
    /// Only `Some` values are cached - `None` results are not cached, allowing the
    /// computation to be retried on subsequent calls.
    ///
    /// # Stampede Protection
    ///
    /// Concurrent calls for the same missing key are coalesced. Only one caller
    /// computes the value while others wait and share the result.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache operation fails or if the leader
    /// task panics.
    ///
    /// # Examples
    ///
    /// ```
    /// use cachelon::{Cache, LoadingCache};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    /// let loader = LoadingCache::new(cache);
    ///
    /// // Returns None without caching
    /// let result = loader
    ///     .optionally_get_or_insert(&"missing".to_string(), || async { None })
    ///     .await?;
    /// assert!(result.is_none());
    ///
    /// // Returns Some and caches
    /// let result = loader
    ///     .optionally_get_or_insert(&"key".to_string(), || async { Some(42) })
    ///     .await?;
    /// assert_eq!(*result.unwrap().value(), 42);
    /// # Ok::<(), cachelon::Error>(())
    /// # });
    /// ```
    pub async fn optionally_get_or_insert<Fut>(
        &self,
        key: &K,
        f: impl FnOnce() -> Fut + Send,
    ) -> Result<Option<CacheEntry<V>>, Error>
    where
        Fut: Future<Output = Option<V>> + Send,
    {
        self.mergers
            .optionally_get_or_insert
            .execute(key, || Box::pin(self.do_optionally_get_or_insert(key, f)))
            .await
            .unwrap_or_else(|panicked| Err(Error::from_source(panicked)))
    }

    async fn do_optionally_get_or_insert<Fut>(&self, key: &K, f: impl FnOnce() -> Fut) -> Result<Option<CacheEntry<V>>, Error>
    where
        Fut: Future<Output = Option<V>>,
    {
        if let Some(entry) = self.cache.get(key).await? {
            return Ok(Some(entry));
        }
        match f().await {
            Some(value) => {
                let entry = CacheEntry::new(value);
                self.cache.insert(key, entry.clone()).await?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }
}

// Delegate basic cache operations to the underlying cache
impl<K, V, S> LoadingCache<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync,
{
    /// Retrieves a value from the cache.
    ///
    /// See [`Cache::get`] for details.
    pub async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        self.cache.get(key).await
    }

    /// Inserts a value into the cache.
    ///
    /// See [`Cache::insert`] for details.
    pub async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.cache.insert(key, entry).await
    }

    /// Invalidates a value from the cache.
    ///
    /// See [`Cache::invalidate`] for details.
    pub async fn invalidate(&self, key: &K) -> Result<(), Error> {
        self.cache.invalidate(key).await
    }

    /// Clears all entries from the cache.
    ///
    /// See [`Cache::clear`] for details.
    pub async fn clear(&self) -> Result<(), Error> {
        self.cache.clear().await
    }

    /// Returns true if the cache contains a value for the given key.
    ///
    /// See [`Cache::contains`] for details.
    pub async fn contains(&self, key: &K) -> Result<bool, Error> {
        self.cache.contains(key).await
    }

    /// Returns the number of entries in the cache.
    ///
    /// See [`Cache::len`] for details.
    #[must_use]
    pub fn len(&self) -> Option<u64> {
        self.cache.len()
    }

    /// Returns true if the cache is empty.
    ///
    /// See [`Cache::is_empty`] for details.
    #[must_use]
    pub fn is_empty(&self) -> Option<bool> {
        self.cache.is_empty()
    }
}
