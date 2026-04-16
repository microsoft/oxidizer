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
    let after_ops = mock_cache_after.operations();
    assert_eq!(after_ops.len(), 2);
    let serialized_key = postcard::to_allocvec(&expected_key).expect("postcard serialization should not fail");
    let serialized_value = postcard::to_allocvec(&expected_value).expect("postcard serialization should not fail");
    assert!(
        matches!(&after_ops[0], CacheOp::Insert { key, entry } if *key == serialized_key.as_slice() && *entry.value() == serialized_value.as_slice())
    );
    assert!(matches!(&after_ops[1], CacheOp::Get(k) if *k == serialized_key.as_slice()));

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

    cache.insert("key".to_string(), "value".to_string()).await.expect("Insert failed");

    // The L3 (post-serialize) tier should have received serialized bytes.
    let l3_ops = l3.operations();
    assert_eq!(l3_ops.len(), 1);
    assert!(matches!(&l3_ops[0], CacheOp::Insert { .. }));
}

#[cfg_attr(miri, ignore)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_serialize_from_multiple_tasks() {
    use std::sync::Arc;

    let mock_after = MockCache::<BytesView, BytesView>::new();
    let cache = Arc::new(
        Cache::builder(Clock::new_frozen())
            .storage(MockCache::<String, String>::new())
            .serialize()
            .fallback(Cache::builder(Clock::new_frozen()).storage(mock_after.clone()))
            .promotion_policy(FallbackPromotionPolicy::always())
            .build(),
    );

    // Spawn multiple tasks that serialize concurrently across different threads.
    let mut handles = Vec::new();
    for i in 0..20 {
        let cache = Arc::clone(&cache);
        handles.push(tokio::spawn(async move {
            let key = format!("key-{i}");
            let value = format!("value-{i}");
            cache.insert(key.clone(), value.clone()).await.expect("Insert failed");
            let result = cache.get(&key).await.expect("Get failed").expect("Should be Some");
            assert_eq!(*result.value(), value);
        }));
    }

    for handle in handles {
        handle.await.expect("Task panicked");
    }

    // All 20 inserts should have reached the post-serialize tier with correct content.
    let after_ops = mock_after.operations();
    let insert_count = after_ops.iter().filter(|op| matches!(op, CacheOp::Insert { .. })).count();
    assert_eq!(insert_count, 20);

    // Verify each insert has the expected serialized key and value.
    for i in 0..20 {
        let expected_key = format!("key-{i}");
        let expected_value = format!("value-{i}");
        let serialized_key = postcard::to_allocvec(&expected_key).expect("postcard serialization should not fail");
        let serialized_value = postcard::to_allocvec(&expected_value).expect("postcard serialization should not fail");
        assert!(
            after_ops.iter().any(|op| matches!(op, CacheOp::Insert { key, entry } if *key == serialized_key.as_slice() && *entry.value() == serialized_value.as_slice())),
            "missing serialized insert for {expected_key}={expected_value}"
        );
    }
}

#[cfg_attr(miri, ignore)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serialize_on_one_thread_deserialize_on_another() {
    use std::sync::Arc;

    let mock_before = MockCache::<String, String>::new();
    let cache = Arc::new(
        Cache::builder(Clock::new_frozen())
            .storage(mock_before.clone())
            .serialize()
            .fallback(Cache::builder(Clock::new_frozen()).storage(MockCache::<BytesView, BytesView>::new()))
            .promotion_policy(FallbackPromotionPolicy::always())
            .build(),
    );

    // Insert on one task (may run on any worker thread).
    let cache_clone = Arc::clone(&cache);
    tokio::spawn(async move {
        cache_clone
            .insert("cross-thread".to_string(), "hello from another task".to_string())
            .await
            .expect("Insert failed");
    })
    .await
    .expect("Insert task panicked");

    // Invalidate from primary so get must deserialize from the serialized tier.
    mock_before
        .invalidate(&"cross-thread".to_string())
        .await
        .expect("Invalidate failed");

    // Get on another task (may run on a different worker thread).
    let cache_clone = Arc::clone(&cache);
    let result = tokio::spawn(async move {
        cache_clone
            .get(&"cross-thread".to_string())
            .await
            .expect("Get failed")
            .expect("Should be Some")
    })
    .await
    .expect("Get task panicked");

    assert_eq!(*result.value(), "hello from another task");
}
