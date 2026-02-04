// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Dynamic cache tier wrapper for type erasure.

use std::{fmt::Debug, sync::Arc};

use crate::{CacheEntry, CacheTier, Error, tier::DynCacheTier};

/// Extension trait for converting any `CacheTier` into a `DynamicCache`.
///
/// This trait is automatically implemented for all types that implement `CacheTier`.
///
/// # Examples
///
/// ```
/// use cachelon_tier::{CacheTier, DynamicCache, DynamicCacheExt};
/// # use cachelon_tier::CacheEntry;
///
/// async fn example<T>(tier: T) -> DynamicCache<String, i32>
/// where
///     T: CacheTier<String, i32> + 'static,
/// {
///     tier.into_dynamic()
/// }
/// ```
pub trait DynamicCacheExt<K, V>: Sized {
    /// Converts this cache tier into a `DynamicCache`.
    fn into_dynamic(self) -> DynamicCache<K, V>;
}

impl<K, V, T> DynamicCacheExt<K, V> for T
where
    T: CacheTier<K, V> + 'static,
{
    fn into_dynamic(self) -> DynamicCache<K, V> {
        DynamicCache::new(self)
    }
}

/// A clonable dynamic cache tier with type erasure.
///
/// `DynamicCache` wraps a trait object in an `Arc` to enable cloning while maintaining
/// dynamic dispatch. Use this when you need to erase the concrete storage type
/// in multi-tier cache hierarchies.
///
/// # Examples
///
/// ```ignore
/// let dynamic: DynamicCache<String, i32> = some_tier.into_dynamic();
///
/// // DynamicCache is Clone
/// let clone = dynamic.clone();
/// ```
pub struct DynamicCache<K, V>(Arc<DynCacheTier<'static, K, V>>);

impl<K, V> DynamicCache<K, V> {
    /// Creates a new dynamic cache from any `CacheTier` implementation.
    pub(crate) fn new<T>(strategy: T) -> Self
    where
        T: CacheTier<K, V> + Send + Sync + 'static,
    {
        Self(DynCacheTier::new_arc(strategy))
    }
}

impl<K, V> Debug for DynamicCache<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DynamicCache").finish()
    }
}

impl<K, V> Clone for DynamicCache<K, V> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<K, V> CacheTier<K, V> for DynamicCache<K, V>
where
    K: Sync,
    V: Send,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        self.0.get(key).await
    }

    async fn insert(&self, key: &K, entry: CacheEntry<V>) -> Result<(), Error> {
        self.0.insert(key, entry).await
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        self.0.invalidate(key).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.0.clear().await
    }

    fn len(&self) -> Option<u64> {
        self.0.len()
    }

    fn is_empty(&self) -> Option<bool> {
        self.0.is_empty()
    }
}
