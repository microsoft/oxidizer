// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for transform module: `TransformAdapter` and `TransformBuilder`.

#![cfg(feature = "test-util")]

use std::time::Duration;

use cachet::{Cache, CacheEntry, CacheOp, CacheTier, IdentityCodec, MockCache, TransformAdapter, TransformCodec, TransformEncoder};
use tick::Clock;

// ── TransformAdapter direct tests ──

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
    assert!(value.is_some());

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
                entry: CacheEntry::new(1),
            },
            CacheOp::Insert {
                key: 2,
                entry: CacheEntry::new(2),
            },
        ]
    );
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn invalidate_maps_key() {
    let inner = MockCache::new();
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    adapter.invalidate(&"42".to_string()).await.unwrap();

    assert_eq!(inner.operations(), vec![CacheOp::Invalidate(42)]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn clear_delegates_to_inner() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    adapter.clear().await.unwrap();

    assert_eq!(inner.operations(), vec![CacheOp::Clear]);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_returns_none_when_inner_misses() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    let result = adapter.get(&"999".to_string()).await.unwrap();
    assert!(result.is_none());
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_preserves_entry_metadata() {
    use std::time::{Duration, SystemTime};

    let now = SystemTime::now();
    let ttl = Duration::from_secs(300);
    let entry = CacheEntry::expires_at(42, ttl, now);
    let data = vec![(1, entry)];
    let inner = MockCache::with_data(data.into_iter().collect());

    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    let result = adapter.get(&"1".to_string()).await.unwrap().unwrap();
    assert_eq!(*result.value(), "42".to_string());
    assert_eq!(result.cached_at(), Some(now));
    assert_eq!(result.ttl(), Some(ttl));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_key_encode_error_propagates() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|_k: &String| "not_a_number".parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    adapter.get(&"bad".to_string()).await.unwrap_err();
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn get_value_decode_error_propagates() {
    let data = vec![(1, CacheEntry::new(1))];
    let inner = MockCache::with_data(data.into_iter().collect());
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            // Decode always fails
            |_v: &i32| Err::<String, _>("bad".parse::<i32>().unwrap_err()),
        ),
    );

    adapter.get(&"1".to_string()).await.unwrap_err();
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn insert_key_encode_error_propagates() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|_k: &String| "not_a_number".parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    adapter
        .insert("bad".to_string(), CacheEntry::new("1".to_string()))
        .await
        .unwrap_err();
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn insert_value_encode_error_propagates() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |_v: &String| "not_a_number".parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    adapter
        .insert("1".to_string(), CacheEntry::new("hello".to_string()))
        .await
        .unwrap_err();
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn invalidate_key_encode_error_propagates() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|_k: &String| "not_a_number".parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    adapter.invalidate(&"bad".to_string()).await.unwrap_err();
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn len_delegates_to_inner() {
    let data = vec![(1, CacheEntry::new(1)), (2, CacheEntry::new(2))];
    let inner = MockCache::with_data(data.into_iter().collect());

    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    assert_eq!(adapter.len(), Some(2));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn identity_roundtrip() {
    let data = vec![(1, CacheEntry::new(100))];
    let inner = MockCache::with_data(data.into_iter().collect());
    let adapter = TransformAdapter::new(inner, IdentityCodec, IdentityCodec);

    let result = adapter.get(&1).await.unwrap().unwrap();
    assert_eq!(*result.value(), 100);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn identity_insert_roundtrip() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(inner.clone(), IdentityCodec, IdentityCodec);

    adapter.insert(1, CacheEntry::new(42)).await.unwrap();

    let result = adapter.get(&1).await.unwrap().unwrap();
    assert_eq!(*result.value(), 42);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn infallible_encoder() {
    let inner = MockCache::<String, i32>::new();
    let adapter = TransformAdapter::new(
        inner.clone(),
        TransformEncoder::infallible(|k: &i32| k.to_string()),
        TransformCodec::new(
            |v: &i32| Ok::<_, std::convert::Infallible>(*v),
            |v: &i32| Ok::<_, std::convert::Infallible>(*v),
        ),
    );

    adapter.insert(42, CacheEntry::new(100)).await.unwrap();

    assert_eq!(
        inner.operations(),
        vec![CacheOp::Insert {
            key: "42".to_string(),
            entry: CacheEntry::new(100),
        }]
    );
}

// ── Debug impls ──

#[test]
fn transform_encoder_debug() {
    let encoder = TransformEncoder::custom(|k: &String| k.parse::<i32>());
    let debug = format!("{encoder:?}");
    assert!(debug.contains("TransformEncoder"));
}

#[test]
fn transform_codec_debug() {
    let codec = TransformCodec::new(
        |v: &String| v.parse::<i32>(),
        |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
    );
    let debug = format!("{codec:?}");
    assert!(debug.contains("TransformCodec"));
}

#[test]
fn transform_adapter_debug() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );
    let debug = format!("{adapter:?}");
    assert!(debug.contains("TransformAdapter"));
}

// ── TransformBuilder tests ──

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn builder_transform_on_cache_builder() {
    let clock = Clock::new_frozen();

    let cache = Cache::builder::<String, String>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .transform(TransformEncoder::infallible(|k: &String| k.clone()), IdentityCodec)
        .fallback(
            Cache::builder::<String, String>(Clock::new_frozen())
                .storage(MockCache::new())
                .ttl(Duration::from_secs(300)),
        )
        .build();

    cache.insert("key".to_string(), CacheEntry::new("value".to_string())).await.unwrap();

    let result = cache.get(&"key".to_string()).await.unwrap();
    assert_eq!(*result.unwrap().value(), "value");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn builder_transform_with_type_mapping() {
    let clock = Clock::new_frozen();

    let l2 = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .ttl(Duration::from_secs(300));

    let cache = Cache::builder::<i32, i32>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .transform(
            TransformEncoder::infallible(|k: &i32| k.to_string()),
            TransformCodec::new(
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
                |v: &String| v.parse::<i32>(),
            ),
        )
        .fallback(l2)
        .build();

    cache.insert(42, CacheEntry::new(100)).await.unwrap();

    let result = cache.get(&42).await.unwrap();
    assert_eq!(*result.unwrap().value(), 100);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn builder_transform_on_fallback_builder() {
    let clock = Clock::new_frozen();

    let l2 = Cache::builder::<i32, i32>(clock.clone())
        .storage(MockCache::new())
        .ttl(Duration::from_secs(300));

    let l3 = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .ttl(Duration::from_secs(600));

    let cache = Cache::builder::<i32, i32>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .fallback(l2)
        .transform(
            TransformEncoder::infallible(|k: &i32| k.to_string()),
            TransformCodec::new(
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
                |v: &String| v.parse::<i32>(),
            ),
        )
        .fallback(l3)
        .build();

    cache.insert(1, CacheEntry::new(10)).await.unwrap();

    let result = cache.get(&1).await.unwrap();
    assert_eq!(*result.unwrap().value(), 10);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn builder_transform_chained_fallback() {
    let clock = Clock::new_frozen();

    let l2_a = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .ttl(Duration::from_secs(300));

    let l2_b = Cache::builder::<String, String>(clock.clone())
        .storage(MockCache::new())
        .ttl(Duration::from_secs(600));

    let cache = Cache::builder::<i32, i32>(clock)
        .memory()
        .ttl(Duration::from_secs(60))
        .transform(
            TransformEncoder::infallible(|k: &i32| k.to_string()),
            TransformCodec::new(
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
                |v: &String| v.parse::<i32>(),
            ),
        )
        .fallback(l2_a)
        .fallback(l2_b)
        .build();

    cache.insert(1, CacheEntry::new(10)).await.unwrap();

    let result = cache.get(&1).await.unwrap();
    assert_eq!(*result.unwrap().value(), 10);
}

#[test]
fn transform_builder_debug() {
    let clock = Clock::new_frozen();

    let builder = Cache::builder::<i32, i32>(clock).memory().ttl(Duration::from_secs(60)).transform(
        TransformEncoder::infallible(|k: &i32| k.to_string()),
        TransformCodec::new(
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
            |v: &String| v.parse::<i32>(),
        ),
    );

    let debug = format!("{builder:?}");
    assert!(debug.contains("TransformBuilder"));
}

// ── CacheEntry::map_value tests ──

#[test]
fn map_value_preserves_metadata() {
    use std::time::{Duration, SystemTime};

    let now = SystemTime::now();
    let ttl = Duration::from_secs(60);
    let entry = CacheEntry::expires_at(42, ttl, now);
    let mapped = entry.map_value(|v| v.to_string());

    assert_eq!(*mapped.value(), "42");
    assert_eq!(mapped.cached_at(), Some(now));
    assert_eq!(mapped.ttl(), Some(ttl));
}

#[test]
fn map_value_without_metadata() {
    let entry = CacheEntry::new(42);
    let mapped = entry.map_value(|v| v * 2);

    assert_eq!(*mapped.value(), 84);
    assert_eq!(mapped.cached_at(), None);
    assert_eq!(mapped.ttl(), None);
}
