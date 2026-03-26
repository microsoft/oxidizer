// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CacheEntry, CacheTier, Codec, Error};

use std::fmt::Debug;

/// A boxed-closure codec for custom transforms.
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
#[derive(Debug, Clone, Copy)]
pub struct IdentityCodec;

impl<T: Clone + Send + Sync> Codec<T, T> for IdentityCodec {
    fn apply(&self, value: &T) -> Result<T, Error> {
        Ok(value.clone())
    }
}

/// Adapter that transforms keys and values between user types and storage types.
///
/// `TransformAdapter<K, KT, V, VT, S>`:
/// - `K, V` = user-facing types (the types the adapter exposes via `CacheTier<K, V>`)
/// - `KT, VT` = storage types (the types used by the inner `S: CacheTier<KT, VT>`)
/// - Codecs go from user → storage: `key_encoder: K→KT`, `value_encoder: V→VT`, `value_decoder: VT→V`
///
/// Implements `CacheTier<K, V>` by encoding keys/values to `KT, VT` for the inner tier.
pub struct TransformAdapter<K, KT, V, VT, S>
where
    S: CacheTier<KT, VT>,
{
    inner: S,
    key_encoder: Box<dyn Codec<K, KT>>,
    value_encoder: Box<dyn Codec<V, VT>>,
    value_decoder: Box<dyn Codec<VT, V>>,
}

impl<K, KT, V, VT, S> TransformAdapter<K, KT, V, VT, S>
where
    S: CacheTier<KT, VT>,
{
    pub fn new(
        inner: S,
        key_encoder: impl Codec<K, KT> + 'static,
        value_encoder: impl Codec<V, VT> + 'static,
        value_decoder: impl Codec<VT, V> + 'static,
    ) -> Self {
        Self {
            inner,
            key_encoder: Box::new(key_encoder),
            value_encoder: Box::new(value_encoder),
            value_decoder: Box::new(value_decoder),
        }
    }
}

impl<K, KT, V, VT, S> CacheTier<K, V> for TransformAdapter<K, KT, V, VT, S>
where
    K: Send + Sync,
    V: Send + Sync,
    KT: Send + Sync,
    VT: Send + Sync,
    S: CacheTier<KT, VT> + Send + Sync,
{
    async fn get(&self, key: &K) -> Result<Option<CacheEntry<V>>, Error> {
        let mapped_key = self.key_encoder.apply(key)?;
        let entry_option = self.inner.get(&mapped_key).await?;
        if let Some(entry) = entry_option {
            let mapped_value = self.value_decoder.apply(entry.value())?;
            Ok(Some(entry.map_value(|_| mapped_value)))
        } else {
            Ok(None)
        }
    }

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
        let mapped_key = self.key_encoder.apply(&key)?;
        let mapped_value = self.value_encoder.apply(entry.value())?;
        let mapped_entry = entry.map_value(|_| mapped_value);
        self.inner.insert(mapped_key, mapped_entry).await
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
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

impl<K, KT, V, VT, S> Debug for TransformAdapter<K, KT, V, VT, S>
where
    S: CacheTier<KT, VT> + Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformAdapter")
            .field("inner", &self.inner)
            .field("K", &std::any::type_name::<K>())
            .field("KT", &std::any::type_name::<KT>())
            .field("V", &std::any::type_name::<V>())
            .field("VT", &std::any::type_name::<VT>())
            .finish()
    }
}
