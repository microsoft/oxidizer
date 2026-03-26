// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `MapAdapter`.

#![cfg(feature = "test-util")]

use cachet::{CacheEntry, CacheOp, CacheTier, Error, MapAdapter, MapCodec, MockCache};

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_returns_mapped_from_inner() {
    let data = vec![(1, CacheEntry::new(1))];
    let inner = MockCache::with_data(data.into_iter().collect());
    let adapter = MapAdapter::new(
        inner.clone(),
        MapCodec::custom(|k: &String| k.parse::<i32>()),
        MapCodec::custom(|v: &String| v.parse::<i32>()),
        MapCodec::infallible(|v: &i32| v.to_string()),
    );

    let value = adapter.get(&"1".to_string()).await.unwrap();

    // Verify operations
    assert_eq!(inner.operations(), vec![CacheOp::Get(1),]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn insert_maps_and_inserts_into_inner() {
    let inner = MockCache::new();
    let adapter = MapAdapter::new(
        inner.clone(),
        MapCodec::custom(|k: &String| k.parse::<i32>()),
        MapCodec::custom(|v: &String| v.parse::<i32>()),
        MapCodec::infallible(|v: &i32| v.to_string()),
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
    let adapter = MapAdapter::new(
        inner.clone(),
        MapCodec::custom(|k: &String| k.parse::<i32>()),
        MapCodec::custom(|v: &String| v.parse::<i32>()),
        MapCodec::infallible(|v: &i32| v.to_string()),
    );
    adapter.invalidate(&"1".to_string()).await.unwrap();

    // Verify operations
    assert_eq!(inner.operations(), vec![CacheOp::Invalidate(1),]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn clear_calls_inner_clear() {
    let inner = MockCache::new();
    let adapter = MapAdapter::new(
        inner.clone(),
        MapCodec::custom(|k: &String| k.parse::<i32>()),
        MapCodec::custom(|v: &String| v.parse::<i32>()),
        MapCodec::infallible(|v: &i32| v.to_string()),
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
    let adapter = MapAdapter::new(
        inner.clone(),
        MapCodec::custom(|k: &String| k.parse::<i32>()),
        MapCodec::custom(|v: &String| v.parse::<i32>()),
        MapCodec::infallible(|v: &i32| v.to_string()),
    );

    let len = adapter.len().await;

    // Verify operations
    assert_eq!(inner.operations(), vec![CacheOp::Len,]);
}
