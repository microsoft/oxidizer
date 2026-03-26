// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CacheEntry, CacheTier, Codec, Error};

use std::fmt::Debug;

/// A boxed-closure codec for custom transforms.
///
/// This is the escape hatch for users who want to use closures rather than
/// implementing the [`Codec`] trait directly. Involves boxing.
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
    fn apply(&self, value: &A) -> Result<B, Error> {
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

/// An identity codec that passes values through unchanged.
///
/// Used as the key codec in compress/encrypt `TransformAdapter` layers
/// where only values need transformation.
#[derive(Debug, Clone, Copy)]
pub struct IdentityCodec;

impl<T: Clone + Send + Sync> Codec<T, T> for IdentityCodec {
    fn apply(&self, value: &T) -> Result<T, Error> {
        Ok(value.clone())
    }
}

/// Adapter that transforms keys and values between two type spaces.
///
/// Generic over codec types to avoid boxing when concrete codecs
/// (e.g., `BincodeCodec`, `ZstdCodec`) are used directly.
pub struct TransformAdapter<K1, K2, V1, V2, S, KE, VE, VD>
where
    S: CacheTier<K2, V2>,
    KE: Codec<K1, K2>,
    VE: Codec<V1, V2>,
    VD: Codec<V2, V1>,
{
    inner: S,
    key_encoder: KE,
    value_encoder: VE,
    value_decoder: VD,
    _phantom: std::marker::PhantomData<(K1, K2, V1, V2)>,
}

impl<K1, K2, V1, V2, S, KE, VE, VD> TransformAdapter<K1, K2, V1, V2, S, KE, VE, VD>
where
    S: CacheTier<K2, V2>,
    KE: Codec<K1, K2>,
    VE: Codec<V1, V2>,
    VD: Codec<V2, V1>,
{
    pub fn new(inner: S, key_encoder: KE, value_encoder: VE, value_decoder: VD) -> Self {
        Self {
            inner,
            key_encoder,
            value_encoder,
            value_decoder,
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<K1, K2, V1, V2, S, KE, VE, VD> CacheTier<K1, V1> for TransformAdapter<K1, K2, V1, V2, S, KE, VE, VD>
where
    K1: Send + Sync,
    V1: Send + Sync,
    K2: Send + Sync,
    V2: Send + Sync,
    S: CacheTier<K2, V2> + Send + Sync,
    KE: Codec<K1, K2>,
    VE: Codec<V1, V2>,
    VD: Codec<V2, V1>,
{
    async fn get(&self, key: &K1) -> Result<Option<CacheEntry<V1>>, Error> {
        let mapped_key = self.key_encoder.apply(key)?;
        let entry_option = self.inner.get(&mapped_key).await?;
        if let Some(entry) = entry_option {
            let mapped_value = self.value_decoder.apply(entry.value())?;
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

impl<K1, K2, V1, V2, S, KE, VE, VD> Debug for TransformAdapter<K1, K2, V1, V2, S, KE, VE, VD>
where
    S: CacheTier<K2, V2> + Debug,
    KE: Codec<K1, K2>,
    VE: Codec<V1, V2>,
    VD: Codec<V2, V1>,
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
