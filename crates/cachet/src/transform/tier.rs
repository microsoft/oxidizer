// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::transform::codec::DecodeOutcome;
use crate::{CacheEntry, CacheTier, Codec, Encoder, Error, SizeError};

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
            let ttl = entry.ttl();
            let cached_at = entry.cached_at();
            let decoded = self.value_codec.decode(entry.into_value())?;
            match decoded {
                DecodeOutcome::Value(v) => {
                    let mut e = CacheEntry::new(v);
                    if let Some(ttl) = ttl {
                        e.set_ttl(ttl);
                    }
                    if let Some(t) = cached_at {
                        e.ensure_cached_at(t);
                    }
                    Ok(Some(e))
                }
                DecodeOutcome::SoftFailure(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
        let mapped_key = self.key_encoder.encode(&key)?;
        let mapped_entry = entry.try_map_value(|v| self.value_codec.encode(&v))?;
        self.inner.insert(mapped_key, mapped_entry).await
    }

    async fn invalidate(&self, key: &K) -> Result<(), Error> {
        let mapped_key = self.key_encoder.encode(key)?;
        self.inner.invalidate(&mapped_key).await
    }

    async fn clear(&self) -> Result<(), Error> {
        self.inner.clear().await
    }

    async fn len(&self) -> Result<u64, SizeError> {
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
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::codec::{TransformCodec, TransformEncoder, infallible_owned};
    use cachet_tier::MockCache;

    #[test]
    fn transform_adapter_debug() {
        let codec = TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        );
        // Exercise both directions so closure bodies are covered.
        assert_eq!(codec.encode(&"42".to_string()).unwrap(), 42);
        assert!(matches!(codec.decode(42).unwrap(), DecodeOutcome::Value(s) if s == "42"));

        let key_encoder = TransformEncoder::new(|k: &String| k.parse::<i32>());
        // Exercise the encoder so the wrapping closure is covered.
        assert_eq!(key_encoder.encode(&"7".to_string()).unwrap(), 7);

        let inner = MockCache::<i32, i32>::new();
        let adapter = TransformAdapter::from_boxed(inner, Box::new(key_encoder), Box::new(codec));
        let debug = format!("{adapter:?}");
        assert!(debug.contains("TransformAdapter"));
    }

    #[test]
    fn infallible_encoder_closure_is_covered() {
        let encoder = TransformEncoder::infallible(|k: &i32| k.to_string());
        assert_eq!(encoder.encode(&42).unwrap(), "42");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn len_delegates_to_inner() {
        use crate::infallible;

        let data = vec![(1, CacheEntry::new(10)), (2, CacheEntry::new(20))];
        let inner = MockCache::with_data(data.into_iter().collect());
        let adapter = TransformAdapter::from_boxed(
            inner,
            Box::new(TransformEncoder::new(|k: &String| k.parse::<i32>())),
            Box::new(TransformCodec::new(infallible(|v: &i32| *v), infallible_owned(|v: i32| v))),
        );
        assert_eq!(adapter.len().await.expect("MockCache::len returns Ok"), 2);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_preserves_ttl_and_cached_at() {
        use std::time::{Duration, SystemTime};

        use crate::infallible;

        let ttl = Duration::from_secs(300);
        let cached_at = SystemTime::now();
        let mut entry = CacheEntry::new(42);
        entry.set_ttl(ttl);
        entry.ensure_cached_at(cached_at);

        let inner = MockCache::with_data(std::iter::once((1, entry)).collect());
        let adapter = TransformAdapter::from_boxed(
            inner,
            Box::new(TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k))),
            Box::new(TransformCodec::new(infallible(|v: &i32| *v), infallible_owned(|v: i32| v))),
        );

        let result = adapter.get(&1).await.unwrap().expect("should be Some");
        assert_eq!(*result.value(), 42);
        assert_eq!(result.ttl(), Some(ttl));
        assert_eq!(result.cached_at(), Some(cached_at));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_returns_none_on_soft_failure() {
        use crate::Encoder as _;

        /// A codec that always returns SoftFailure on decode.
        struct FailCodec;

        impl crate::Encoder<i32, i32> for FailCodec {
            fn encode(&self, value: &i32) -> Result<i32, crate::Error> {
                Ok(*value)
            }
        }

        impl crate::Codec<i32, i32> for FailCodec {
            fn decode(&self, _value: i32) -> Result<DecodeOutcome<i32>, crate::Error> {
                Ok(DecodeOutcome::SoftFailure("test failure"))
            }
        }

        impl std::fmt::Debug for FailCodec {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("FailCodec")
            }
        }

        let inner = MockCache::with_data(std::iter::once((1, CacheEntry::new(42))).collect());
        let adapter = TransformAdapter::from_boxed(
            inner,
            Box::new(TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k))),
            Box::new(FailCodec),
        );

        let result = adapter.get(&1).await.unwrap();
        assert!(result.is_none(), "soft failure should return None");
    }
}
