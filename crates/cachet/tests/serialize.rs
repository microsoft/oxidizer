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
        .insert(expected_key.clone(), expected_value.clone().into())
        .await
        .expect("Insert failed");

    // Remove from the pre-transform cache to force the get to hit the post-transform cache.
    mock_cache_before
        .invalidate(&expected_key)
        .await
        .expect("Invalidate failed");

    // Get the value — this should deserialize from the post-transform cache.
    let actual_value = cache
        .get(&expected_key)
        .await
        .expect("Should be Ok")
        .expect("Should be Some");

    // Verify the pre-transform cache saw the correct operations with original types.
    let before_ops = mock_cache_before.operations();
    assert_eq!(before_ops.len(), 3);
    assert!(matches!(&before_ops[0], CacheOp::Insert { key, entry } if key == &expected_key && entry.value() == &expected_value));
    assert!(matches!(&before_ops[1], CacheOp::Invalidate(k) if k == &expected_key));
    assert!(matches!(&before_ops[2], CacheOp::Get(k) if k == &expected_key));

    // Verify the post-transform cache received serialized operations.
    let after_ops = mock_cache_after.operations();
    assert_eq!(after_ops.len(), 2);
    assert!(matches!(&after_ops[0], CacheOp::Insert { .. }));
    assert!(matches!(&after_ops[1], CacheOp::Get(_)));

    // Verify the round-trip: value was serialized, stored, fetched, and deserialized correctly.
    assert_eq!(*actual_value.value(), expected_value);
}
