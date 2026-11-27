// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serialization contracts through public APIs.

#![cfg(test)]

use bytesbuf::mem::GlobalPool;
use std::sync::LazyLock;
testing_aids::init_tracing!();

#[cfg(feature = "serialize")]
static MEMORY: LazyLock<GlobalPool> = LazyLock::new(GlobalPool::new);

#[cfg(feature = "serialize")]
mod serialization {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytesbuf::BytesView;
    use bytesbuf::mem::{CallbackMemory, OpaqueMemory};
    use cachet::{Cache, CacheEntry, CacheOp, CacheTier, Error};
    use cachet_tier::MockCache;
    use serde::{Deserialize, Serialize};
    use thread_aware::unaware;
    use tick::Clock;

    #[derive(Clone, Deserialize)]
    struct FailSerialize;

    impl Serialize for FailSerialize {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional failure"))
        }
    }

    fn counting_memory() -> (OpaqueMemory, Arc<AtomicUsize>) {
        let reservations = Arc::new(AtomicUsize::new(0));
        let provider = CallbackMemory::new(unaware(Arc::clone(&reservations)), |counter, min_bytes| {
            counter.fetch_add(1, Ordering::SeqCst);
            crate::MEMORY.reserve(min_bytes)
        });
        (OpaqueMemory::new(provider), reservations)
    }

    fn encode<T>(value: T, memory: OpaqueMemory) -> Result<BytesView, Error>
    where
        T: Clone + Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        futures::executor::block_on(async {
            let clock = Clock::new_frozen();
            let storage = MockCache::<BytesView, BytesView>::new();
            let cache = Cache::builder::<u32, T>(clock.clone())
                .storage(MockCache::new())
                .serialize(memory)
                .fallback(Cache::builder(clock).storage(storage.clone()))
                .build();
            cache.insert(1, CacheEntry::new(value)).await?;
            let Some(CacheOp::Insert { entry, .. }) = storage.operations().into_iter().find(|op| matches!(op, CacheOp::Insert { .. }))
            else {
                panic!("successful serialized insert must reach storage");
            };
            Ok(entry.into_value())
        })
    }

    fn decode(value: BytesView) -> String {
        futures::executor::block_on(async {
            let clock = Clock::new_frozen();
            let storage = MockCache::<BytesView, BytesView>::new();
            let mut key = vec![1];
            key.extend(postcard::to_allocvec(&1u32).unwrap());
            storage.insert(BytesView::from(key), CacheEntry::new(value)).await.unwrap();
            let cache = Cache::builder::<u32, String>(clock.clone())
                .storage(MockCache::new())
                .serialize(crate::MEMORY.clone())
                .fallback(Cache::builder(clock).storage(storage))
                .build();
            cache.get(&1).await.unwrap().unwrap().into_value()
        })
    }

    #[test]
    fn encode_serialization_failure_returns_err() {
        encode(FailSerialize, crate::MEMORY.clone().into()).unwrap_err();
    }

    #[test]
    fn encode_decode_roundtrip() {
        let original = "hello, world!".to_string();
        let encoded = encode(original.clone(), crate::MEMORY.clone().into()).unwrap();
        assert_eq!(decode(encoded), original);
    }

    #[test]
    fn encoder_encode_produces_valid_output() {
        let clock = Clock::new_frozen();
        let storage = MockCache::<BytesView, BytesView>::new();
        let cache = Cache::builder::<u32, u32>(clock.clone())
            .storage(MockCache::new())
            .serialize(crate::MEMORY.clone())
            .fallback(Cache::builder(clock).storage(storage.clone()))
            .build();
        assert!(futures::executor::block_on(cache.get(&42)).unwrap().is_none());
        let Some(CacheOp::Get(encoded)) = storage.operations().into_iter().next() else {
            panic!("serialized cache miss must query storage");
        };
        let bytes = encoded.to_vec();
        assert_eq!(bytes[0], 1, "first byte should be format version");
        assert_eq!(postcard::from_bytes::<u32>(&bytes[1..]).unwrap(), 42);
    }

    #[test]
    fn encoder_uses_supplied_memory_provider() {
        let (memory, reservations) = counting_memory();
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<u32, u32>(clock.clone())
            .storage(MockCache::new())
            .serialize(memory)
            .fallback(Cache::builder(clock).storage(MockCache::<BytesView, BytesView>::new()))
            .build();
        assert!(futures::executor::block_on(cache.get(&42)).unwrap().is_none());
        assert!(reservations.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn codec_uses_supplied_memory_provider() {
        let (memory, reservations) = counting_memory();
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<u32, u32>(clock.clone())
            .storage(MockCache::new())
            .serialize(memory)
            .fallback(Cache::builder(clock).storage(MockCache::<BytesView, BytesView>::new()))
            .build();
        assert!(futures::executor::block_on(cache.get(&1)).unwrap().is_none());
        let key_reservations = reservations.load(Ordering::SeqCst);
        futures::executor::block_on(cache.insert(1, CacheEntry::new(42))).unwrap();
        assert!(
            reservations.load(Ordering::SeqCst) > key_reservations.saturating_mul(2),
            "value encoding must reserve memory in addition to the two key encodings"
        );
    }

    #[test]
    fn decode_multi_span_view() {
        let original = "multi-span test".to_string();
        let bytes = encode(original.clone(), crate::MEMORY.clone().into()).unwrap().to_vec();
        let mid = bytes.len() / 2;
        let mut first_half = BytesView::from(bytes[..mid].to_vec());
        first_half.append(BytesView::from(bytes[mid..].to_vec()));
        assert_ne!(first_half.first_slice().len(), first_half.len(), "should be multi-span");
        assert_eq!(decode(first_half), original);
    }
}
