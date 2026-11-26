// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Transformed storage through public APIs.

#![cfg(test)]

testing_aids::init_tracing!();

mod transform_tier {
    fn transform_cache<K, V, KT, VT>(
        inner: cachet_tier::MockCache<KT, VT>,
        encoder: impl cachet::Encoder<K, KT> + 'static,
        codec: impl cachet::Codec<V, VT> + 'static,
    ) -> Cache<K, V>
    where
        K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
        V: Clone + Send + Sync + 'static,
        KT: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
        VT: Clone + Send + Sync + 'static,
    {
        let clock = Clock::new_frozen();
        Cache::builder(clock.clone())
            .storage(cachet_tier::MockCache::<K, V>::new())
            .transform(encoder, codec)
            .fallback(Cache::builder(clock).storage(inner))
            .build()
    }

    #[derive(Debug)]
    struct SoftFailureCodec;
    impl Encoder<i32, i32> for SoftFailureCodec {
        fn encode(&self, value: &i32) -> Result<i32, cachet::Error> {
            Ok(*value)
        }
    }
    impl Codec<i32, i32> for SoftFailureCodec {
        fn decode(&self, _: i32) -> Result<DecodeOutcome<i32>, cachet::Error> {
            Ok(DecodeOutcome::SoftFailure("test failure"))
        }
    }

    use cachet::{Cache, CacheEntry, CacheTier, Codec, DecodeOutcome, Encoder, TransformCodec, TransformEncoder, infallible_owned};
    use cachet_tier::MockCache;
    use tick::Clock;

    #[test]
    fn infallible_encoder_closure_is_covered() {
        let encoder = TransformEncoder::infallible(|k: &i32| k.to_string());
        assert_eq!(encoder.encode(&42).unwrap(), "42");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_delegates_to_inner() {
        let inner = MockCache::<i32, i32>::new();
        let inner_check = inner.clone();
        let adapter = transform_cache(
            inner,
            TransformEncoder::new(|k: &String| k.parse::<i32>()),
            TransformCodec::new(|v: &String| v.parse::<i32>(), infallible_owned(|v: i32| v.to_string())),
        );

        adapter.insert("42".to_string(), CacheEntry::new("100".to_string())).await.unwrap();

        let stored = inner_check.get(&42).await.unwrap().unwrap();
        assert_eq!(*stored.value(), 100);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_preserves_ttl_and_cached_at() {
        use std::time::Duration;

        use cachet::infallible;

        let ttl = Duration::from_mins(5);
        let cached_at = Clock::new_frozen().system_time();
        let mut entry = CacheEntry::new(42);
        entry.set_ttl(ttl);
        entry.ensure_cached_at(cached_at);

        let inner = MockCache::with_data(std::iter::once((1, entry)).collect());
        let adapter = transform_cache(
            inner,
            TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k)),
            TransformCodec::new(infallible(|v: &i32| *v), infallible_owned(|v: i32| v)),
        );

        let result = adapter.get(&1).await.unwrap().unwrap();
        assert_eq!(*result.value(), 42);
        assert_eq!(result.ttl(), Some(ttl));
        assert_eq!(result.cached_at(), Some(cached_at));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn get_returns_none_on_soft_failure() {
        let inner = MockCache::with_data(std::iter::once((1, CacheEntry::new(42))).collect());
        let adapter = transform_cache(
            inner,
            TransformEncoder::new(|k: &i32| Ok::<_, std::convert::Infallible>(*k)),
            SoftFailureCodec,
        );

        let result = adapter.get(&1).await.unwrap();
        assert!(result.is_none(), "soft failure should return None");
    }
}
