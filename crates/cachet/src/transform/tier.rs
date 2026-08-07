// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::transform::codec::{CodecContext, DecodeOutcome};
use crate::{CacheEntry, CacheTier, Codec, Encoder, Error, SizeError};

/// Builds the per-operation [`CodecContext`] from a mapped storage key.
///
/// Returns a keyless context for a plain type transform, or a key-bound context for the
/// serialize/protect boundary (where the storage key is bytes and doubles as associated
/// data). It yields the whole [`CodecContext`] — not just the key — so new context fields
/// can be added on the struct without changing this signature; boxed so both forms share
/// one adapter type.
pub(crate) type MakeContext<KT> = Box<dyn for<'a> Fn(&'a KT) -> CodecContext<'a> + Send + Sync>;

/// A [`MakeContext`] that binds no key — the codec receives a keyless context.
pub(crate) fn keyless_context<KT>() -> MakeContext<KT> {
    Box::new(|_| CodecContext::keyless())
}

/// Adapter that transforms keys and values between user types and storage types.
///
/// `TransformAdapter<K, KT, V, VT, S>`:
/// - `K, V` are the user-facing types (the types the adapter exposes via `CacheTier<K, V>`)
/// - `KT, VT` are the storage types (the types used by the inner `S: CacheTier<KT, VT>`)
/// - `key_encoder: K->KT` (one-directional), `value_codec: V<->VT` (bidirectional)
///
/// `make_context` builds the [`CodecContext`] passed to the value codec: keyless for a
/// plain transform, or key-bound for an authenticated (serialize/protect) boundary. The
/// adapter is a pure transform: on read a [`DecodeOutcome::SoftFailure`] simply becomes a
/// cache miss. Any observability of *why* (e.g. an authentication failure) is owned by
/// the value codec that detected it, not by this general adapter.
pub(crate) struct TransformAdapter<K, KT, V, VT, S>
where
    S: CacheTier<KT, VT>,
{
    inner: S,
    key_encoder: Box<dyn Encoder<K, KT>>,
    value_codec: Box<dyn Codec<V, VT>>,
    make_context: MakeContext<KT>,
}

impl<K, KT, V, VT, S> TransformAdapter<K, KT, V, VT, S>
where
    S: CacheTier<KT, VT>,
{
    /// Creates a new `TransformAdapter` from pre-boxed codecs and a context builder.
    pub(crate) fn from_boxed(
        inner: S,
        key_encoder: Box<dyn Encoder<K, KT>>,
        value_codec: Box<dyn Codec<V, VT>>,
        make_context: MakeContext<KT>,
    ) -> Self {
        Self {
            inner,
            key_encoder,
            value_codec,
            make_context,
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
        let Some(entry) = self.inner.get(&mapped_key).await? else {
            return Ok(None);
        };
        let ttl = entry.ttl();
        let cached_at = entry.cached_at();
        let stored = entry.into_value();

        let ctx = (self.make_context)(&mapped_key);
        match self.value_codec.decode(&ctx, stored)? {
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
            DecodeOutcome::SoftFailure => Ok(None),
        }
    }

    async fn insert(&self, key: K, entry: CacheEntry<V>) -> Result<(), Error> {
        let mapped_key = self.key_encoder.encode(&key)?;
        let mapped_entry = {
            let ctx = (self.make_context)(&mapped_key);
            entry.try_map_value(|v| self.value_codec.encode(&ctx, &v))?
        };
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use bytesbuf::BytesView;
    use cachet_tier::MockCache;

    use super::*;
    use crate::transform::codec::{TransformCodec, TransformEncoder, infallible_owned};
    use crate::transform::testing::MockCodec;

    #[test]
    fn transform_adapter_debug() {
        let codec = TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        );
        // Exercise both directions so closure bodies are covered.
        assert_eq!(codec.encode(&CodecContext::keyless(), &"42".to_string()).unwrap(), 42);
        assert!(matches!(codec.decode(&CodecContext::keyless(), 42).unwrap(), DecodeOutcome::Value(s) if s == "42"));

        let key_encoder = TransformEncoder::new(|k: &String| k.parse::<i32>());
        // Exercise the encoder so the wrapping closure is covered.
        assert_eq!(key_encoder.encode(&"7".to_string()).unwrap(), 7);

        let inner = MockCache::<i32, i32>::new();
        let adapter = TransformAdapter::from_boxed(inner, Box::new(key_encoder), Box::new(codec), keyless_context());
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
            keyless_context(),
        );
        assert_eq!(adapter.len().await.expect("MockCache::len returns Ok"), 2);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_preserves_ttl_and_cached_at() {
        use std::time::{Duration, SystemTime};

        use crate::infallible;

        let ttl = Duration::from_mins(5);
        let cached_at = SystemTime::now();
        let mut entry = CacheEntry::new(42);
        entry.set_ttl(ttl);
        entry.ensure_cached_at(cached_at);

        let inner = MockCache::with_data(std::iter::once((1, entry)).collect());
        let adapter = TransformAdapter::from_boxed(
            inner,
            Box::new(TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k))),
            Box::new(TransformCodec::new(infallible(|v: &i32| *v), infallible_owned(|v: i32| v))),
            keyless_context(),
        );

        let result = adapter.get(&1).await.unwrap().expect("should be Some");
        assert_eq!(*result.value(), 42);
        assert_eq!(result.ttl(), Some(ttl));
        assert_eq!(result.cached_at(), Some(cached_at));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn soft_failure_reads_as_a_miss() {
        // The adapter is a pure transform: a decode soft-failure is just a miss, with no
        // telemetry of its own (that belongs to whichever codec detected the failure).
        let inner = MockCache::with_data(std::iter::once((1, CacheEntry::new(42))).collect());
        let adapter = TransformAdapter::from_boxed(
            inner,
            Box::new(TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k))),
            Box::new(MockCodec::<i32>::soft_failure()),
            keyless_context(),
        );

        assert!(adapter.get(&1).await.unwrap().is_none(), "a soft failure reads as a miss");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn key_context_binds_the_mapped_key() {
        // A codec whose decode returns the context key proves the adapter threads the
        // mapped key bytes into the codec context (a keyless context would yield empty).
        let inner = MockCache::<BytesView, BytesView>::new();
        let adapter = TransformAdapter::from_boxed(
            inner,
            Box::new(TransformEncoder::new(|k: &BytesView| Ok::<_, std::convert::Infallible>(k.clone()))),
            Box::new(KeyReportingCodec),
            Box::new(|key: &BytesView| CodecContext::from_key(std::borrow::Cow::Owned(key.to_vec()))),
        );

        adapter
            .insert(BytesView::from(b"k".to_vec()), CacheEntry::new(BytesView::from(b"v".to_vec())))
            .await
            .expect("insert should succeed");
        let got = adapter
            .get(&BytesView::from(b"k".to_vec()))
            .await
            .expect("get should succeed")
            .expect("present");
        assert_eq!(got.value().to_vec(), b"k", "decode must observe the bound key via the context");
    }

    /// A codec that stores the value as-is but decodes to the context key, so a test can
    /// observe which key the adapter bound into the context.
    struct KeyReportingCodec;

    impl Codec<BytesView, BytesView> for KeyReportingCodec {
        fn encode(&self, _ctx: &CodecContext<'_>, value: &BytesView) -> Result<BytesView, Error> {
            Ok(value.clone())
        }

        fn decode(&self, ctx: &CodecContext<'_>, _value: BytesView) -> Result<DecodeOutcome<BytesView>, Error> {
            Ok(DecodeOutcome::Value(BytesView::from(ctx.key().to_vec())))
        }
    }
}
