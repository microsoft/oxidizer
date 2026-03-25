// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use cachet_tier::{CacheEntry, CacheTier, Error};

pub struct Mapper<T1, T2> {
    f: Box<dyn Fn(T1) -> T2 + Send + Sync>,
}

impl<T1: 'static, T2: From<T1> + 'static> Mapper<T1, T2> {
    pub fn from_impl() -> Self {
        Self { f: Box::new(T2::from) }
    }
}

impl<T1, T2> Mapper<T1, T2> {
    pub fn custom(f: impl Fn(T1) -> T2 + Send + Sync + 'static) -> Self {
        Self { f: Box::new(f) }
    }

    fn map(&self, value: &T1) -> T2 {
        (self.f)(value)
    }
}

impl<T1: 'static, T2: From<T1> + 'static> Default for Mapper<T1, T2> {
    fn default() -> Self {
        Self::from_impl()
    }
}

pub struct MapAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2>,
{
    inner: S,
    key_into: Mapper<K1, K2>,
    value_into: Mapper<V1, V2>,
    value_from: Mapper<V2, V1>,
}

impl<K1, K2, V1, V2, S> MapAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2>,
{
    pub fn new(inner: S, key_into: Mapper<K1, K2>, value_into: Mapper<V1, V2>, value_from: Mapper<V2, V1>) -> Self {
        Self {
            inner,
            key_into,
            value_into,
            value_from,
        }
    }
}

impl<K1, K2, V1, V2, S> CacheTier<K1, V1> for MapAdapter<K1, K2, V1, V2, S>
where
    K1: Send + Sync,
    V1: Send + Sync,
    K2: Send + Sync,
    V2: Send + Sync,
    S: CacheTier<K2, V2> + Send + Sync,
{
    async fn get(&self, key: &K1) -> Result<Option<CacheEntry<V1>>, Error> {
        let entry_option = self.inner.get(&self.key_into.map(key)).await?;
        Ok(entry_option.map(|entry| entry.map_value(|v| self.value_from.map(&v))))
    }

    async fn insert(&self, key: K1, value: CacheEntry<V1>) -> Result<(), Error> {
        self.inner.insert(self.key_into.map(&key), self.value_into.map(&value)).await
    }

    async fn invalidate(&self, key: &K1) -> Result<(), Error> {
        self.inner.invalidate(&self.key_into.map(key)).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.inner.clear().await
    }

    fn len(&self) -> Option<u64> {
        self.inner.len()
    }
}
