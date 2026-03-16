// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Dynamic cache tier wrapper for type erasure.

use std::fmt::Debug;
use std::sync::Arc;

use crate::tier::DynCacheTier;
use crate::{CacheEntry, CacheTier, Error};

/// A clonable dynamic cache tier with type erasure.
///
/// `DynamicCache` wraps a trait object in an `Arc` to enable cloning while maintaining
/// dynamic dispatch. Use this when you need to erase the concrete storage type
/// in multi-tier cache hierarchies.
///
/// # Examples
///
/// ```ignore
/// let dynamic = DynamicCache::new(some_tier);
///
/// // DynamicCache is Clone
/// let clone = dynamic.clone();
/// ```
pub struct DynamicCache<K, V>(Arc<DynCacheTier<'static, K, V>>);

impl<K, V> DynamicCache<K, V> {
    /// Creates a new dynamic cache from any `CacheTier` implementation.
    pub fn new<T>(strategy: T) -> Self
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

impl<K: Sync, V: Send> CacheTier<K, V> for DynamicCache<K, V> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockCache;

    #[tokio::test]
    async fn clone_shares_state() {
        let cache = MockCache::<String, i32>::new();
        let dynamic = DynamicCache::new(cache);
        let clone = dynamic.clone();

        dynamic.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

        let entry = clone.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);
    }
}
