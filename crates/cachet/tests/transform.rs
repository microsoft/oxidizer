// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the transform module via `CacheBuilder`.

#![cfg(feature = "test-util")]

use std::time::Duration;

use cachet::{Cache, CacheEntry, CacheOp, MockCache, TransformCodec, TransformEncoder, infallible, infallible_owned};
use tick::Clock;

/// Builds a cache with L1 (`MockCache<String, String>`) and L2 (`MockCache<i32, i32>`)
/// separated by a String-to-i32 transform boundary.
fn build_transform_cache(
    clock: Clock,
    l1: MockCache<String, String>,
    l2: MockCache<i32, i32>,
) -> Cache<String, String, impl cachet::CacheTier<String, String>> {
    Cache::builder::<String, String>(clock.clone())
        .storage(l1)
        .transform(
            TransformEncoder::new(|k: &String| k.parse::<i32>()),
            TransformCodec::new(|v: &String| v.parse::<i32>(), infallible_owned(|v: i32| v.to_string())),
        )
        .fallback(Cache::builder::<i32, i32>(clock).storage(l2))
        .build()
}

// -- Insert + Get through transform boundary --

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn insert_transforms_into_l2() {
    let clock = Clock::new_frozen();
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<i32, i32>::new();
    let cache = build_transform_cache(clock, l1, l2.clone());

    cache.insert("42".to_string(), CacheEntry::new("100".to_string())).await.unwrap();

    let l2_ops = l2.operations();
    let insert_ops: Vec<_> = l2_ops.iter().filter(|op| matches!(op, CacheOp::Insert { .. })).collect();
    assert_eq!(insert_ops.len(), 1);
    assert!(matches!(&insert_ops[0], CacheOp::Insert { key: 42, .. }));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_falls_back_through_transform() {
    let clock = Clock::new_frozen();
    let l1 = MockCache::<String, String>::new();
    let l2_data = vec![(42, CacheEntry::new(100))];
    let l2 = MockCache::with_data(l2_data.into_iter().collect());
    let cache = build_transform_cache(clock, l1, l2);

    let result = cache.get(&"42".to_string()).await.unwrap().unwrap();
    assert_eq!(*result.value(), "100");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_miss_in_both_tiers() {
    let clock = Clock::new_frozen();
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<i32, i32>::new();
    let cache = build_transform_cache(clock, l1, l2);

    let result = cache.get(&"999".to_string()).await.unwrap();
    assert!(result.is_none());
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn invalidate_transforms_key_to_l2() {
    let clock = Clock::new_frozen();
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<i32, i32>::new();
    let cache = build_transform_cache(clock, l1, l2.clone());

    cache.invalidate(&"42".to_string()).await.unwrap();

    let l2_ops = l2.operations();
    assert!(l2_ops.contains(&CacheOp::Invalidate(42)));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn clear_propagates_to_l2() {
    let clock = Clock::new_frozen();
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<i32, i32>::new();
    let cache = build_transform_cache(clock, l1, l2.clone());

    cache.clear().await.unwrap();

    let l2_ops = l2.operations();
    assert!(l2_ops.contains(&CacheOp::Clear));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn roundtrip_insert_then_get() {
    let clock = Clock::new_frozen();
    let l1 = MockCache::<String, String>::new();
    let l2 = MockCache::<i32, i32>::new();
    let cache = build_transform_cache(clock, l1, l2);

    cache.insert("1".to_string(), CacheEntry::new("42".to_string())).await.unwrap();

    let result = cache.get(&"1".to_string()).await.unwrap().unwrap();
    assert_eq!(*result.value(), "42");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn len_delegates_through_transform() {
    let clock = Clock::new_frozen();
    let l1_data = vec![
        ("1".to_string(), CacheEntry::new("a".to_string())),
        ("2".to_string(), CacheEntry::new("b".to_string())),
    ];
    let l1 = MockCache::with_data(l1_data.into_iter().collect());
    let l2 = MockCache::<i32, i32>::new();
    let cache = build_transform_cache(clock, l1, l2);

    let len = cache.len();
    assert_eq!(len, Some(2));
}

// -- Error propagation --

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_key_encode_error_propagates() {
    let clock = Clock::new_frozen();

    let cache = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .transform(
            TransformEncoder::new(|_k: &String| "bad".parse::<i32>()),
            TransformCodec::new(|v: &String| v.parse::<i32>(), infallible_owned(|v: i32| v.to_string())),
        )
        .fallback(Cache::builder::<i32, i32>(clock).storage(MockCache::new()))
        .build();

    let err = cache.get(&"anything".to_string()).await.unwrap_err();
    assert!(err.is_source::<std::num::ParseIntError>());
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_value_decode_error_propagates() {
    let clock = Clock::new_frozen();
    let l2_data = vec![(1, CacheEntry::new(1))];

    let cache = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .transform(
            TransformEncoder::new(|k: &String| k.parse::<i32>()),
            TransformCodec::new(
                |v: &String| v.parse::<i32>(),
                |_v: i32| Err::<String, _>("bad".parse::<i32>().unwrap_err()),
            ),
        )
        .fallback(Cache::builder::<i32, i32>(clock).storage(MockCache::with_data(l2_data.into_iter().collect())))
        .build();

    let err = cache.get(&"1".to_string()).await.unwrap_err();
    assert!(err.is_source::<std::num::ParseIntError>());
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn insert_key_encode_error_propagates() {
    let clock = Clock::new_frozen();

    let cache = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .transform(
            TransformEncoder::new(|_k: &String| "bad".parse::<i32>()),
            TransformCodec::new(|v: &String| v.parse::<i32>(), infallible_owned(|v: i32| v.to_string())),
        )
        .fallback(Cache::builder::<i32, i32>(clock).storage(MockCache::new()))
        .build();

    let err = cache.insert("1".to_string(), CacheEntry::new("42".to_string())).await.unwrap_err();
    assert!(err.is_source::<std::num::ParseIntError>());
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn insert_value_encode_error_propagates() {
    let clock = Clock::new_frozen();

    let cache = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .transform(
            TransformEncoder::new(|k: &String| k.parse::<i32>()),
            TransformCodec::new(|_v: &String| "bad".parse::<i32>(), infallible_owned(|v: i32| v.to_string())),
        )
        .fallback(Cache::builder::<i32, i32>(clock).storage(MockCache::new()))
        .build();

    let err = cache
        .insert("1".to_string(), CacheEntry::new("hello".to_string()))
        .await
        .unwrap_err();
    assert!(err.is_source::<std::num::ParseIntError>());
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn invalidate_key_encode_error_propagates() {
    let clock = Clock::new_frozen();

    let cache = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .transform(
            TransformEncoder::new(|_k: &String| "bad".parse::<i32>()),
            TransformCodec::new(|v: &String| v.parse::<i32>(), infallible_owned(|v: i32| v.to_string())),
        )
        .fallback(Cache::builder::<i32, i32>(clock).storage(MockCache::new()))
        .build();

    let err = cache.invalidate(&"bad".to_string()).await.unwrap_err();
    assert!(err.is_source::<std::num::ParseIntError>());
}

// -- Infallible encoder through builder --

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn infallible_encoder_through_builder() {
    let clock = Clock::new_frozen();
    let l2 = MockCache::<String, i32>::new();

    let cache = Cache::builder::<i32, i32>(clock.clone())
        .storage(MockCache::new())
        .transform(
            TransformEncoder::infallible(|k: &i32| k.to_string()),
            TransformCodec::new(infallible(|v: &i32| *v), infallible_owned(|v: i32| v)),
        )
        .fallback(Cache::builder::<String, i32>(clock).storage(l2.clone()))
        .build();

    cache.insert(42, CacheEntry::new(100)).await.unwrap();

    let l2_ops = l2.operations();
    let insert_ops: Vec<_> = l2_ops.iter().filter(|op| matches!(op, CacheOp::Insert { .. })).collect();
    assert_eq!(insert_ops.len(), 1);
    assert!(matches!(&insert_ops[0], CacheOp::Insert { key, .. } if key == "42"));
}

// -- FallbackBuilder.transform() --

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_on_fallback_builder() {
    let clock = Clock::new_frozen();

    let cache = Cache::builder::<i32, i32>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(Cache::builder::<i32, i32>(clock.clone()).storage(MockCache::new()))
        .transform(
            TransformEncoder::infallible(|k: &i32| k.to_string()),
            TransformCodec::new(infallible(|v: &i32| v.to_string()), |v: String| v.parse::<i32>()),
        )
        .fallback(Cache::builder::<String, String>(clock).storage(MockCache::new()))
        .build();

    cache.insert(1, CacheEntry::new(10)).await.unwrap();

    let result = cache.get(&1).await.unwrap().unwrap();
    assert_eq!(*result.value(), 10);
}

// -- Chained post-transform fallback --

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn chained_post_transform_fallback() {
    let clock = Clock::new_frozen();

    let cache = Cache::builder::<i32, i32>(clock.clone())
        .memory()
        .ttl(Duration::from_secs(60))
        .transform(
            TransformEncoder::infallible(|k: &i32| k.to_string()),
            TransformCodec::new(infallible(|v: &i32| v.to_string()), |v: String| v.parse::<i32>()),
        )
        .fallback(Cache::builder::<String, String>(clock.clone()).storage(MockCache::new()))
        .fallback(Cache::builder::<String, String>(clock).storage(MockCache::new()))
        .build();

    cache.insert(1, CacheEntry::new(10)).await.unwrap();

    let result = cache.get(&1).await.unwrap().unwrap();
    assert_eq!(*result.value(), 10);
}

// -- Debug impls --

#[test]
fn transform_encoder_debug() {
    let encoder = TransformEncoder::new(|k: &String| k.parse::<i32>());
    let debug = format!("{encoder:?}");
    assert!(debug.contains("TransformEncoder"));
}

#[test]
fn transform_codec_debug() {
    let codec = TransformCodec::new(|v: &String| v.parse::<i32>(), infallible_owned(|v: i32| v.to_string()));
    let debug = format!("{codec:?}");
    assert!(debug.contains("TransformCodec"));
}

#[cfg_attr(miri, ignore)]
#[test]
fn transform_builder_debug() {
    let clock = Clock::new_frozen();
    let builder = Cache::builder::<i32, i32>(clock).memory().ttl(Duration::from_secs(60)).transform(
        TransformEncoder::infallible(|k: &i32| k.to_string()),
        TransformCodec::new(infallible(|v: &i32| v.to_string()), |v: String| v.parse::<i32>()),
    );
    let debug = format!("{builder:?}");
    assert!(debug.contains("TransformBuilder"));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_builder_time_to_refresh() {
    let l1 = MockCache::<i32, i32>::new();
    let l2 = MockCache::<String, String>::new();

    let refresh = cachet::TimeToRefresh::new(Duration::from_secs(30), anyspawn::Spawner::new_tokio());
    let _cache = Cache::builder(Clock::new_frozen())
        .storage(l1)
        .transform(
            TransformEncoder::infallible(|k: &i32| k.to_string()),
            TransformCodec::new(infallible(|v: &i32| v.to_string()), |v: String| v.parse::<i32>()),
        )
        .fallback(Cache::builder(Clock::new_frozen()).storage(l2))
        .promotion_policy(cachet::FallbackPromotionPolicy::always())
        .time_to_refresh(refresh)
        .build();
}

// -- CacheEntry::map_value tests --

#[cfg_attr(miri, ignore)]
#[test]
fn try_map_value_preserves_metadata() {
    use std::time::{Duration, SystemTime};

    let now = SystemTime::now();
    let ttl = Duration::from_secs(60);
    let entry = CacheEntry::expires_at(42, ttl, now);
    let mapped = entry.try_map_value(|v| Ok(v.to_string())).expect("Returns Ok");

    assert_eq!(*mapped.value(), "42");
    assert_eq!(mapped.cached_at(), Some(now));
    assert_eq!(mapped.ttl(), Some(ttl));
}

#[test]
fn try_map_value_without_metadata() {
    let entry = CacheEntry::new(42);
    let mapped = entry.try_map_value(|v| Ok(v * 2)).expect("Returns Ok");

    assert_eq!(*mapped.value(), 84);
    assert_eq!(mapped.cached_at(), None);
    assert_eq!(mapped.ttl(), None);
}
