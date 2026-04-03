// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CacheEntry, CacheTier, Codec, Encoder, Error};

use std::fmt::Debug;

type EncodeFn<A, B> = Box<dyn Fn(&A) -> Result<B, Error> + Send + Sync>;

/// A boxed-closure encoder for custom one-directional transforms (keys).
pub struct TransformEncoder<A, B> {
    encode_fn: EncodeFn<A, B>,
}

impl<A, B> TransformEncoder<A, B> {
    /// Creates a new `TransformEncoder` from a fallible closure.
    pub fn custom<EncodeError>(encode_fn: impl Fn(&A) -> Result<B, EncodeError> + Send + Sync + 'static) -> Self
    where
        EncodeError: std::error::Error + Send + Sync + 'static,
    {
        Self {
            encode_fn: Box::new(move |a| encode_fn(a).map_err(|e| Error::from_source(e))),
        }
    }

    /// Creates a new `TransformEncoder` from an infallible closure.
    pub fn infallible(encode_fn: impl Fn(&A) -> B + Send + Sync + 'static) -> Self {
        Self {
            encode_fn: Box::new(move |a| Ok(encode_fn(a))),
        }
    }
}

impl<A, B> Encoder<A, B> for TransformEncoder<A, B> {
    fn encode(&self, value: &A) -> Result<B, Error> {
        (self.encode_fn)(value)
    }
}

impl<A, B> Debug for TransformEncoder<A, B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformEncoder")
            .field("A", &std::any::type_name::<A>())
            .field("B", &std::any::type_name::<B>())
            .finish()
    }
}

/// A boxed-closure codec for custom bidirectional transforms (values).
pub struct TransformCodec<A, B> {
    encode_fn: EncodeFn<A, B>,
    decode_fn: EncodeFn<B, A>,
}

impl<A, B> TransformCodec<A, B> {
    /// Creates a new `TransformCodec` from a pair of fallible closures.
    pub fn new<EncodeError, DecodeError>(
        encode_fn: impl Fn(&A) -> Result<B, EncodeError> + Send + Sync + 'static,
        decode_fn: impl Fn(&B) -> Result<A, DecodeError> + Send + Sync + 'static,
    ) -> Self
    where
        EncodeError: std::error::Error + Send + Sync + 'static,
        DecodeError: std::error::Error + Send + Sync + 'static,
    {
        Self {
            encode_fn: Box::new(move |a| encode_fn(a).map_err(|e| Error::from_source(e))),
            decode_fn: Box::new(move |b| decode_fn(b).map_err(|e| Error::from_source(e))),
        }
    }
}

impl<A, B> Encoder<A, B> for TransformCodec<A, B> {
    fn encode(&self, value: &A) -> Result<B, Error> {
        (self.encode_fn)(value)
    }
}

impl<A, B> Codec<A, B> for TransformCodec<A, B> {
    fn decode(&self, value: &B) -> Result<A, Error> {
        (self.decode_fn)(value)
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

impl<T: Clone + Send + Sync> Encoder<T, T> for IdentityCodec {
    fn encode(&self, value: &T) -> Result<T, Error> {
        Ok(value.clone())
    }
}

impl<T: Clone + Send + Sync> Codec<T, T> for IdentityCodec {
    fn decode(&self, value: &T) -> Result<T, Error> {
        Ok(value.clone())
    }
}

/// Adapter that transforms keys and values between user types and storage types.
///
/// `TransformAdapter<K, KT, V, VT, S>`:
/// - `K, V` are the user-facing types (the types the adapter exposes via `CacheTier<K, V>`)
/// - `KT, VT` are the storage types (the types used by the inner `S: CacheTier<KT, VT>`)
/// - `key_encoder: K->KT` (one-directional), `value_codec: V<->VT` (bidirectional)
///
/// Implements `CacheTier<K, V>` by encoding keys/values to `KT, VT` for the inner tier.
pub(crate) struct TransformAdapter<K, KT, V, VT, S>
where
    S: CacheTier<KT, VT>,
{
    inner: S,
    key_encoder: Box<dyn Encoder<K, KT>>,
    value_codec: Box<dyn Codec<V, VT>>,
}

impl<K, KT, V, VT, S> TransformAdapter<K, KT, V, VT, S>
where
    S: CacheTier<KT, VT>,
{
    /// Creates a new `TransformAdapter` from pre-boxed codecs.
    pub(crate) fn from_boxed(inner: S, key_encoder: Box<dyn Encoder<K, KT>>, value_codec: Box<dyn Codec<V, VT>>) -> Self {
        Self {
            inner,
            key_encoder,
            value_codec,
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
        let mapped_key = self.key_encoder.encode(key)?;
        let entry_option = self.inner.get(&mapped_key).await?;
        if let Some(entry) = entry_option {
            let mapped_value = self.value_codec.decode(entry.value())?;
            Ok(Some(entry.map_value(|_| mapped_value)))
        } else {
            Ok(None)
        }
    }

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
        let mapped_key = self.key_encoder.encode(&key)?;
        let mapped_value = self.value_codec.encode(entry.value())?;
        let mapped_entry = entry.map_value(|_| mapped_value);
        self.inner.insert(mapped_key, mapped_entry).await
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let mapped_key = self.key_encoder.encode(key)?;
        self.inner.invalidate(&mapped_key).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.inner.clear().await
    }

    fn len(&self) -> Option<u64> {
        self.inner.len()
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
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cachet_tier::MockCache;

    #[test]
    fn transform_adapter_debug() {
        let inner = MockCache::<i32, i32>::new();
        let adapter = TransformAdapter::from_boxed(
            inner,
            Box::new(TransformEncoder::custom(|k: &String| k.parse::<i32>())),
            Box::new(TransformCodec::new(
                |v: &String| v.parse::<i32>(),
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
            )),
        );
        let debug = format!("{adapter:?}");
        assert!(debug.contains("TransformAdapter"));
    }
}
