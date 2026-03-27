// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `TransformAdapter`.

#![cfg(feature = "test-util")]

use cachet::{CacheEntry, CacheOp, CacheTier, MockCache, TransformAdapter, TransformCodec, TransformEncoder};

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_returns_mapped_from_inner() {
    let data = vec![(1, CacheEntry::new(1))];
    let inner = MockCache::with_data(data.into_iter().collect());
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    let value = adapter.get(&"1".to_string()).await.unwrap();

    // Verify operations
    assert_eq!(inner.operations(), vec![CacheOp::Get(1),]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn insert_maps_and_inserts_into_inner() {
    let inner = MockCache::new();
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );
    adapter.insert("1".to_string(), "1".to_string().into()).await.unwrap();
    adapter.insert("2".to_string(), "2".to_string().into()).await.unwrap();

    // Verify operations
    assert_eq!(
        inner.operations(),
        vec![
            CacheOp::Insert {
                key: 1,
                entry: CacheEntry::new(1)
            },
            CacheOp::Insert {
                key: 2,
                entry: CacheEntry::new(2)
            },
        ]
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn invalidate_maps_and_invalidates_inner() {
    let inner = MockCache::new();
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );
    adapter.invalidate(&"1".to_string()).await.unwrap();

    // Verify operations
    assert_eq!(inner.operations(), vec![CacheOp::Invalidate(1),]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn clear_calls_inner_clear() {
    let inner = MockCache::new();
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );
    adapter.clear().await.unwrap();

    // Verify operations
    assert_eq!(inner.operations(), vec![CacheOp::Clear,]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn len_calls_inner_len() {
    let data = vec![(1, CacheEntry::new(1)), (2, CacheEntry::new(2))];
    let inner = MockCache::with_data(data.into_iter().collect());
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    let len = adapter.len().await;

    // Verify operations
    assert_eq!(inner.operations(), vec![CacheOp::Len,]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_builder_with_fallback() {
    use cachet::Cache;
    use tick::Clock;

    let clock = Clock::new_frozen();

    // Pre-transform: memory cache with String keys, i32 values
    // Post-transform: mock cache with i32 keys, String values
    let remote = Cache::builder::<i32, String>(clock.clone()).storage(MockCache::new());

    let cache = Cache::builder::<String, i32>(clock)
        .storage(MockCache::new())
        .transform(
            TransformEncoder::custom(|k: &String| k.parse::<i32>()),
            TransformCodec::new(
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
                |v: &String| v.parse::<i32>(),
            ),
        )
        .fallback(remote)
        .build();

    // Insert and retrieve
    cache.insert("42".to_string(), CacheEntry::new(42)).await.unwrap();
    let result = cache.get(&"42".to_string()).await.unwrap();
    assert!(result.is_some());
    assert_eq!(*result.unwrap().value(), 42);
}
