// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CacheEntry, CacheTier, Codec, Error};

use std::fmt::Debug;

pub struct TransformCodec<A, B> {
    apply_fn: Box<dyn Fn(&A) -> Result<B, Error> + Send + Sync>,
}

impl<A, B> TransformCodec<A, B> {
    pub fn custom<ApplyError>(apply_fn: impl Fn(&A) -> Result<B, ApplyError> + Send + Sync + 'static) -> Self
    where
        ApplyError: std::error::Error + Send + Sync + 'static,
    {
        Self {
            apply_fn: Box::new(move |a| apply_fn(a).map_err(|e| Error::from_source(e))),
        }
    }

    pub fn infallible(apply_fn: impl Fn(&A) -> B + Send + Sync + 'static) -> Self {
        Self {
            apply_fn: Box::new(move |a| Ok(apply_fn(a))),
        }
    }
}

impl<A, B> Codec<A, B> for TransformCodec<A, B> {
    type Error = Error;

    fn apply(&self, value: &A) -> Result<B, Self::Error> {
        (self.apply_fn)(value)
    }
}

impl<A, B> Debug for TransformCodec<A, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformCodec")
            .field("A", &std::any::type_name::<A>())
            .field("B", &std::any::type_name::<B>())
            .finish()
    }
}

pub struct TransformAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2>,
{
    inner: S,
    key_encoder: Box<dyn Codec<K1, K2, Error = Error>>,
    value_encoder: Box<dyn Codec<V1, V2, Error = Error>>,
    value_decoder: Box<dyn Codec<V2, V1, Error = Error>>,
}

impl<K1, K2, V1, V2, S> TransformAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2>,
    K1: 'static,
    K2: 'static,
    V1: 'static,
    V2: 'static,
{
    pub fn new(
        inner: S,
        key_encoder: TransformCodec<K1, K2>,
        value_encoder: TransformCodec<V1, V2>,
        value_decoder: TransformCodec<V2, V1>,
    ) -> Self {
        Self {
            inner,
            key_encoder: Box::new(key_encoder),
            value_encoder: Box::new(value_encoder),
            value_decoder: Box::new(value_decoder),
        }
    }
}

impl<K1, K2, V1, V2, S> CacheTier<K1, V1> for TransformAdapter<K1, K2, V1, V2, S>
where
    K1: Send + Sync,
    V1: Send + Sync,
    K2: Send + Sync,
    V2: Send + Sync,
    S: CacheTier<K2, V2> + Send + Sync,
{
    async fn get(&self, key: &K1) -> Result<Option<CacheEntry<V1>>, Error> {
        let mapped_key = self.key_encoder.apply(key)?;
        let entry_option = self.inner.get(&mapped_key).await?;
        if let Some(entry) = entry_option {
            let mapped_value = self.value_decoder.apply(&entry.value())?;
            Ok(Some(entry.map_value(|_| mapped_value)))
        } else {
            Ok(None)
        }
    }

    async fn insert(&self, key: K1, entry: CacheEntry<V1>) -> Result<(), Error> {
        let mapped_key = self.key_encoder.apply(&key)?;
        let mapped_value = self.value_encoder.apply(entry.value())?;
        let mapped_entry = entry.map_value(|_| mapped_value);
        self.inner.insert(mapped_key, mapped_entry).await
    }

    async fn invalidate(&self, key: &K1) -> Result<(), Error> {
        let mapped_key = self.key_encoder.apply(key)?;
        self.inner.invalidate(&mapped_key).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.inner.clear().await
    }

    async fn len(&self) -> Result<Option<u64>, Error> {
        self.inner.len().await
    }
}

impl<K1, K2, V1, V2, S> Debug for TransformAdapter<K1, K2, V1, V2, S>
where
    S: CacheTier<K2, V2> + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformAdapter")
            .field("inner", &self.inner)
            .field("K1", &std::any::type_name::<K1>())
            .field("K2", &std::any::type_name::<K2>())
            .field("V1", &std::any::type_name::<V1>())
            .field("V2", &std::any::type_name::<V2>())
            .finish()
    }
}
