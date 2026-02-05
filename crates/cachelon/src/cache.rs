// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The main cache type with telemetry and stampede protection.

use std::{fmt::Debug, hash::Hash};

use tick::Clock;
use uniflight::Merger;

use crate::{Error, builder::CacheBuilder};
use cachelon_tier::{CacheEntry, CacheTier};

/// Type alias for cache names used in telemetry.
pub type CacheName = &'static str;

/// Mergers for stampede protection on basic cache operations.
///
/// Each operation type has its own merger to handle the appropriate return type.
/// Concurrent calls for the same key are coalesced - only one executes while
/// others wait and share the result.
struct Mergers<K, V> {
    /// For `get` - returns cached entry or None
    get: Merger<K, Result<Option<CacheEntry<V>>, Error>>,
    /// For `invalidate` - returns unit on success
    invalidate: Merger<K, Result<(), Error>>,
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
/// - Optional stampede protection for `get` and `invalidate` operations
/// - Clock management for time-based operations
///
/// For "get or compute" operations (`get_or_insert`, `try_get_or_insert`, etc.),
/// use [`LoadingCache`](crate::LoadingCache) which wraps a `Cache` and provides
/// these methods with stampede protection.
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
/// cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;
/// let value = cache.get(&"key".to_string()).await?;
/// assert_eq!(*value.unwrap().value(), 42);
/// # Ok::<(), cachelon::Error>(())
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
///     .fallback(l2)
///     .promotion_policy(FallbackPromotionPolicy::always())
///     .build();
/// # });
/// ```
#[derive(Debug)]
pub struct Cache<K, V, S = ()> {
    pub(crate) name: CacheName,
    pub(crate) storage: S,
    pub(crate) clock: Clock,
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

/// Constructor and access methods.
impl<K, V, S> Cache<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync,
{
    pub(crate) fn new(name: CacheName, storage: S, clock: Clock, stampede_protection: bool) -> Self {
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
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: CacheTier<K, V> + Send + Sync,
{
    /// Retrieves a value from the cache.
    ///
    /// Returns `None` if the key is not found or the entry has expired.
    ///
    /// # Stampede Protection
    ///
    /// When enabled via [`stampede_protection()`](crate::builder::CacheBuilder::stampede_protection),
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
    /// ```
    /// use cachelon::{Cache, CacheEntry};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// let result = cache.get(&"missing".to_string()).await?;
    /// assert!(result.is_none());
    /// # Ok::<(), cachelon::Error>(())
    /// # });
    /// ```
    pub async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        if let Some(ref mergers) = self.mergers {
            return mergers
                .get
                .execute(key, || self.storage.get(key))
                .await
                .unwrap_or_else(|panicked| Err(Error::from_source(panicked)));
        }

        self.storage.get(key).await
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
    /// ```
    /// use cachelon::{Cache, CacheEntry};
    /// use tick::Clock;
    /// # futures::executor::block_on(async {
    ///
    /// let clock = Clock::new_frozen();
    /// let cache = Cache::builder::<String, i32>(clock).memory().build();
    ///
    /// cache.insert(&"key".to_string(), CacheEntry::new(42)).await?;
    /// # Ok::<(), cachelon::Error>(())
    /// # });
    /// ```
    pub async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.storage.insert(key, entry).await
    }

    /// Invalidates (removes) a value from the cache.
    ///
    /// # Stampede Protection
    ///
    /// When enabled, concurrent invalidations for the same key are merged.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails.
    pub async fn invalidate(&self, key: &K) -> Result<(), Error> {
        if let Some(ref mergers) = self.mergers {
            return mergers
                .invalidate
                .execute(key, || self.storage.invalidate(key))
                .await
                .unwrap_or_else(|panicked| Err(Error::from_source(panicked)));
        }

        self.storage.invalidate(key).await
    }

    /// Returns true if the cache contains a value for the given key.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying cache tier operation fails.
    pub async fn contains(&self, key: &K) -> Result<bool, Error> {
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
                let entry = self.get(&req.key).await?;
                Ok(cachelon_service::CacheResponse::Get(entry))
            }
            cachelon_service::CacheOperation::Insert(req) => {
                self.insert(&req.key, req.entry).await?;
                Ok(cachelon_service::CacheResponse::Insert())
            }
            cachelon_service::CacheOperation::Invalidate(req) => {
                self.invalidate(&req.key).await?;
                Ok(cachelon_service::CacheResponse::Invalidate())
            }
            cachelon_service::CacheOperation::Clear => {
                self.clear().await?;
                Ok(cachelon_service::CacheResponse::Clear())
            }
        }
    }
}
