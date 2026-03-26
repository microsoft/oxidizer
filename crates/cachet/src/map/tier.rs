// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CacheEntry, CacheTier, Codec, Error};

use std::fmt::Debug;

pub struct MapCodec<A, B> {
    map_fn: Box<dyn Fn(&A) -> Result<B, Error> + Send + Sync>,
}

impl<A, B> MapCodec<A, B> {
    pub fn custom<MapError>(map_fn: impl Fn(&A) -> Result<B, MapError> + Send + Sync + 'static) -> Self
    where
        MapError: std::error::Error + Send + Sync + 'static,
    {
        Self {
            map_fn: Box::new(move |a| map_fn(a).map_err(|e| Error::from_source(e))),
        }
    }

    pub fn infallible(map_fn: impl Fn(&A) -> B + Send + Sync + 'static) -> Self {
        Self {
            map_fn: Box::new(move |a| Ok(map_fn(a))),
        }
    }
}

impl<A, B> Codec<A, B> for MapCodec<A, B> {
    type Error = Error;

    fn map(&self, value: &A) -> Result<B, Self::Error> {
        (self.map_fn)(value)
    }
}

impl<A, B> Debug for MapCodec<A, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Print mapper with A -> B type info
        f.debug_struct("Mapper")
            .field("A", &std::any::type_name::<A>())
            .field("B", &std::any::type_name::<B>())
            .finish()
    }
}

pub struct MapAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2>,
{
    inner: S,
    key_encoder: Box<dyn Codec<K1, K2, Error = Error>>,
    value_encoder: Box<dyn Codec<V1, V2, Error = Error>>,
    value_decoder: Box<dyn Codec<V2, V1, Error = Error>>,
}

impl<K1, K2, V1, V2, S> MapAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2>,
    K1: 'static,
    K2: 'static,
    V1: 'static,
    V2: 'static,
{
    pub fn new(inner: S, key_encoder: MapCodec<K1, K2>, value_encoder: MapCodec<V1, V2>, value_decoder: MapCodec<V2, V1>) -> Self {
        Self {
            inner,
            key_encoder: Box::new(key_encoder),
            value_encoder: Box::new(value_encoder),
            value_decoder: Box::new(value_decoder),
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
        let mapped_key = self.key_encoder.map(key)?;
        let entry_option = self.inner.get(&mapped_key).await?;
        if let Some(entry) = entry_option {
            let mapped_value = self.value_decoder.map(&entry.value())?;
            Ok(Some(entry.map_value(|_| mapped_value)))
        } else {
            Ok(None)
        }
    }

    async fn insert(&self, key: K1, entry: CacheEntry<V1>) -> Result<(), Error> {
        let mapped_key = self.key_encoder.map(&key)?;
        let mapped_value = self.value_encoder.map(entry.value())?;
        let mapped_entry = entry.map_value(|_| mapped_value);
        self.inner.insert(mapped_key, mapped_entry).await
    }

    async fn invalidate(&self, key: &K1) -> Result<(), Error> {
        let mapped_key = self.key_encoder.map(key)?;
        self.inner.invalidate(&mapped_key).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.inner.clear().await
    }

    async fn len(&self) -> Result<Option<u64>, Error> {
        self.inner.len().await
    }
}

impl<K1, K2, V1, V2, S> Debug for MapAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2> + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapAdapter")
            .field("inner", &self.inner)
            .field("K1", &std::any::type_name::<K1>())
            .field("K2", &std::any::type_name::<K2>())
            .field("V1", &std::any::type_name::<V1>())
            .field("V2", &std::any::type_name::<V2>())
            .finish()
    }
}
