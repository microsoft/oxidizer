// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the Serialization API.

#![cfg(feature = "serialize")]

use bytesbuf::BytesView;
use cachet::{Cache, CacheOp, CacheTier, FallbackPromotionPolicy, MockCache};
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn serialize_encode_decode_returns_correct_output() {
    let mock_cache_before = MockCache::<String, String>::new();
    let mock_cache_after = MockCache::<BytesView, BytesView>::new();
    let cache = Cache::builder(Clock::new_frozen())
        .storage(mock_cache_before.clone())
        .serialize()
        .fallback(Cache::builder(Clock::new_frozen()).storage(mock_cache_after.clone()))
        .promotion_policy(FallbackPromotionPolicy::never())
        .build();
    let expected_key = "greeting".to_string();
    let expected_value = "Hello, world!".to_string();
    cache
        .insert(expected_key.clone(), expected_value.clone())
        .await
        .expect("Insert failed");

    // Remove from the pre-transform cache to force the get to hit the post-transform cache.
    mock_cache_before.invalidate(&expected_key).await.expect("Invalidate failed");

    // Get the value — this should deserialize from the post-transform cache.
    let actual_value = cache.get(&expected_key).await.expect("Should be Ok").expect("Should be Some");

    // Verify the pre-transform cache saw the correct operations with original types.
    let before_ops = mock_cache_before.operations();
    assert_eq!(before_ops.len(), 3);
    assert!(matches!(&before_ops[0], CacheOp::Insert { key, entry } if key == &expected_key && entry.value() == &expected_value));
    assert!(matches!(&before_ops[1], CacheOp::Invalidate(k) if k == &expected_key));
    assert!(matches!(&before_ops[2], CacheOp::Get(k) if k == &expected_key));

    // Verify the post-transform cache received serialized operations.
    // The expected bytes are postcard-serialized: varint length prefix + UTF-8 payload.
    let after_ops = mock_cache_after.operations();
    assert_eq!(after_ops.len(), 2);
    let serialized_key: &[u8] = &[8, 103, 114, 101, 101, 116, 105, 110, 103];
    let serialized_value: &[u8] = &[13, 72, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33];
    assert!(matches!(&after_ops[0], CacheOp::Insert { key, entry } if key == &serialized_key && entry.value() == &serialized_value));
    assert!(matches!(&after_ops[1], CacheOp::Get(k) if k == &serialized_key));

    // Verify the round-trip: value was serialized, stored, fetched, and deserialized correctly.
    assert_eq!(*actual_value.value(), expected_value);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn serialize_on_fallback_builder() {
    // Test .serialize() on a FallbackBuilder (not just CacheBuilder).
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<String, String>::new();
    let l3 = MockCache::<BytesView, BytesView>::new();

    let cache = Cache::builder(Clock::new_frozen())
        .storage(l1)
        .fallback(Cache::builder(Clock::new_frozen()).storage(l2))
        .serialize()
        .fallback(Cache::builder(Clock::new_frozen()).storage(l3.clone()))
        .build();

    cache
        .insert("key".to_string(), "value".to_string())
        .await
        .expect("Insert failed");

    // The L3 (post-serialize) tier should have received serialized bytes.
    let l3_ops = l3.operations();
    assert_eq!(l3_ops.len(), 1);
    assert!(matches!(&l3_ops[0], CacheOp::Insert { .. }));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_builder_time_to_refresh() {
    use std::time::Duration;

    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<BytesView, BytesView>::new();

    // Verify time_to_refresh can be set on the transform builder without panicking.
    let refresh = cachet::TimeToRefresh::new(Duration::from_secs(30), anyspawn::Spawner::new_tokio());
    let _cache = Cache::builder(Clock::new_frozen())
        .storage(l1)
        .serialize()
        .fallback(Cache::builder(Clock::new_frozen()).storage(l2))
        .promotion_policy(FallbackPromotionPolicy::always())
        .time_to_refresh(refresh)
        .build();
}
