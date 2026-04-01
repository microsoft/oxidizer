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
async fn encode_error_propagates() {
    let inner = MockCache::<i32, i32>::new();
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|_k: &String| "not_a_number".parse::<i32>()),
        TransformCodec::new(
            |v: &String| v.parse::<i32>(),
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        ),
    );

    let result = adapter.get(&"bad".to_string()).await;
    result.unwrap_err();
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
    use cachet::IdentityCodec;

    let data = vec![(1, CacheEntry::new(100))];
    let inner = MockCache::with_data(data.into_iter().collect());
    let adapter = TransformAdapter::new(inner, IdentityCodec, IdentityCodec);

    let result = adapter.get(&1).await.unwrap().unwrap();
    assert_eq!(*result.value(), 100);
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
