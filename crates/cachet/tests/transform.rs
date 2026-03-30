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

// ---------------------------------------------------------------------------
// IdentityCodec tests
// ---------------------------------------------------------------------------

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn identity_codec_encode_returns_same_value() {
    use cachet::{Encoder, IdentityCodec};

    let codec = IdentityCodec;
    let value = "hello".to_string();
    let encoded = Encoder::<String, String>::encode(&codec, &value).unwrap();
    assert_eq!(encoded, value);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn identity_codec_decode_returns_same_value() {
    use cachet::{Codec, IdentityCodec};

    let codec = IdentityCodec;
    let value = 42i64;
    let decoded = Codec::<i64, i64>::decode(&codec, &value).unwrap();
    assert_eq!(decoded, value);
}

// ---------------------------------------------------------------------------
// TransformEncoder / TransformCodec tests
// ---------------------------------------------------------------------------

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_encoder_custom_success() {
    use cachet::Encoder;

    let encoder = TransformEncoder::custom(|k: &String| k.parse::<i32>());
    let result = Encoder::<String, i32>::encode(&encoder, &"123".to_string()).unwrap();
    assert_eq!(result, 123);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_encoder_custom_propagates_error() {
    use cachet::Encoder;

    let encoder = TransformEncoder::custom(|k: &String| k.parse::<i32>());
    let result = Encoder::<String, i32>::encode(&encoder, &"not_a_number".to_string());
    assert!(result.is_err());
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_encoder_infallible() {
    use cachet::Encoder;

    let encoder = TransformEncoder::infallible(|k: &i32| k.to_string());
    let result = Encoder::<i32, String>::encode(&encoder, &42).unwrap();
    assert_eq!(result, "42");
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_codec_round_trip() {
    use cachet::{Codec, Encoder};

    let codec = TransformCodec::new(
        |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        |v: &String| v.parse::<i32>(),
    );
    let encoded = Encoder::<i32, String>::encode(&codec, &99).unwrap();
    assert_eq!(encoded, "99");
    let decoded = Codec::<i32, String>::decode(&codec, &encoded).unwrap();
    assert_eq!(decoded, 99);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn transform_codec_error_propagation() {
    use cachet::Codec;

    let codec = TransformCodec::new(
        |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        |v: &String| v.parse::<i32>(),
    );
    let result = Codec::<i32, String>::decode(&codec, &"not_a_number".to_string());
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// TransformBuilder edge case tests
// ---------------------------------------------------------------------------

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn builder_multiple_post_transform_fallbacks() {
    use cachet::Cache;
    use tick::Clock;

    let clock = Clock::new_frozen();

    let remote_a = Cache::builder::<i32, String>(clock.clone()).storage(MockCache::new());
    let remote_b = Cache::builder::<i32, String>(clock.clone()).storage(MockCache::new());

    let cache = Cache::builder::<String, i32>(clock)
        .storage(MockCache::new())
        .transform(
            TransformEncoder::custom(|k: &String| k.parse::<i32>()),
            TransformCodec::new(
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
                |v: &String| v.parse::<i32>(),
            ),
        )
        .fallback(remote_a)
        .fallback(remote_b)
        .build();

    cache.insert("7".to_string(), CacheEntry::new(7)).await.unwrap();
    let result = cache.get(&"7".to_string()).await.unwrap();
    assert!(result.is_some());
    assert_eq!(*result.unwrap().value(), 7);
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn builder_nested_transform_as_fallback() {
    use cachet::Cache;
    use tick::Clock;

    let clock = Clock::new_frozen();

    // Inner builder: i32 keys, String values with an identity transform and a remote fallback.
    // This is itself a TransformBuilder, which implements CacheTierBuilder.
    let inner = Cache::builder::<i32, String>(clock.clone())
        .storage(MockCache::new())
        .transform(
            TransformEncoder::infallible(|k: &i32| *k),
            TransformCodec::new(
                |v: &String| Ok::<_, std::convert::Infallible>(v.clone()),
                |v: &String| Ok::<_, std::convert::Infallible>(v.clone()),
            ),
        )
        .fallback(Cache::builder::<i32, String>(clock.clone()).storage(MockCache::new()));

    // Outer cache wraps the inner builder as a fallback
    let cache = Cache::builder::<String, i32>(clock)
        .storage(MockCache::new())
        .transform(
            TransformEncoder::custom(|k: &String| k.parse::<i32>()),
            TransformCodec::new(
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
                |v: &String| v.parse::<i32>(),
            ),
        )
        .fallback(inner)
        .build();

    cache.insert("10".to_string(), CacheEntry::new(10)).await.unwrap();
    let result = cache.get(&"10".to_string()).await.unwrap();
    assert!(result.is_some());
    assert_eq!(*result.unwrap().value(), 10);
}
// ---------------------------------------------------------------------------
// FallbackBuilder.transform() (builder/transform.rs:91-113)
// ---------------------------------------------------------------------------

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn fallback_builder_transform() {
    use cachet::Cache;
    use tick::Clock;

    let clock = Clock::new_frozen();

    // Post-transform remote tier (i32 keys, String values)
    let remote = Cache::builder::<i32, String>(clock.clone()).storage(MockCache::new());

    // L2 memory tier (String keys, i32 values)
    let l2 = Cache::builder::<String, i32>(clock.clone()).storage(MockCache::new());

    // Build: L1 (memory) -> fallback L2 -> .transform() -> fallback remote
    let cache = Cache::builder::<String, i32>(clock)
        .storage(MockCache::new())
        .fallback(l2)
        .transform(
            TransformEncoder::custom(|k: &String| k.parse::<i32>()),
            TransformCodec::new(
                |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
                |v: &String| v.parse::<i32>(),
            ),
        )
        .fallback(remote)
        .build();

    cache.insert("99".to_string(), CacheEntry::new(99)).await.unwrap();
    let result = cache.get(&"99".to_string()).await.unwrap();
    assert!(result.is_some());
    assert_eq!(*result.unwrap().value(), 99);
}
// ---------------------------------------------------------------------------
// Debug impls (tier.rs:39-44, 83-88, 193-201; encrypt.rs:32-34)
// ---------------------------------------------------------------------------

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn debug_impls_for_transform_types() {
    // TransformEncoder Debug (tier.rs:38-45)
    let encoder = TransformEncoder::infallible(|v: &i32| v.to_string());
    let debug_str = format!("{:?}", encoder);
    assert!(debug_str.contains("TransformEncoder"));

    // TransformCodec Debug (tier.rs:82-89)
    let codec = TransformCodec::new(
        |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
        |v: &String| v.parse::<i32>(),
    );
    let debug_str = format!("{:?}", codec);
    assert!(debug_str.contains("TransformCodec"));

    // TransformAdapter Debug (tier.rs:189-202)
    let inner = MockCache::<i32, String>::new();
    let adapter = TransformAdapter::new(
        inner,
        TransformEncoder::custom(|k: &String| k.parse::<i32>()),
        TransformCodec::new(
            |v: &i32| Ok::<_, std::convert::Infallible>(v.to_string()),
            |v: &String| v.parse::<i32>(),
        ),
    );
    let debug_str = format!("{:?}", adapter);
    assert!(debug_str.contains("TransformAdapter"));
}

// ---------------------------------------------------------------------------
// BincodeCodec round-trip tests (serialize feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "serialize")]
mod serialize_tests {
    use cachet::{BincodeCodec, BincodeEncoder, BytesView, CacheEntry, Codec, Encoder, MockCache};

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct UserProfile {
        name: String,
        age: u32,
        tags: Vec<String>,
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn bincode_round_trip_string() {
        let codec = BincodeCodec;
        let original = "hello world".to_string();
        let encoded: BytesView = Encoder::<String, BytesView>::encode(&codec, &original).unwrap();
        let decoded: String = Codec::<String, BytesView>::decode(&codec, &encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn bincode_round_trip_integer() {
        let codec = BincodeCodec;
        let original: i64 = -9_999_999;
        let encoded: BytesView = Encoder::<i64, BytesView>::encode(&codec, &original).unwrap();
        let decoded: i64 = Codec::<i64, BytesView>::decode(&codec, &encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn bincode_round_trip_struct() {
        let codec = BincodeCodec;
        let original = UserProfile {
            name: "Alice".into(),
            age: 30,
            tags: vec!["admin".into(), "active".into()],
        };
        let encoded: BytesView = Encoder::<UserProfile, BytesView>::encode(&codec, &original).unwrap();
        let decoded: UserProfile = Codec::<UserProfile, BytesView>::decode(&codec, &encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn bincode_round_trip_empty_vec() {
        let codec = BincodeCodec;
        let original: Vec<u8> = vec![];
        let encoded: BytesView = Encoder::<Vec<u8>, BytesView>::encode(&codec, &original).unwrap();
        let decoded: Vec<u8> = Codec::<Vec<u8>, BytesView>::decode(&codec, &encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn bincode_round_trip_large_value() {
        let codec = BincodeCodec;
        let original: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let encoded: BytesView = Encoder::<Vec<u8>, BytesView>::encode(&codec, &original).unwrap();
        let decoded: Vec<u8> = Codec::<Vec<u8>, BytesView>::decode(&codec, &encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn bytesview_serialize_round_trip_preserves_data() {
        let codec = BincodeCodec;
        let original = UserProfile {
            name: "Bob".into(),
            age: 25,
            tags: vec!["user".into()],
        };
        let bytes: BytesView = Encoder::<UserProfile, BytesView>::encode(&codec, &original).unwrap();
        let cloned = bytes.clone();
        let decoded: UserProfile = Codec::<UserProfile, BytesView>::decode(&codec, &cloned).unwrap();
        assert_eq!(decoded, original);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn bincode_encoder_produces_non_empty_output() {
        let encoder = BincodeEncoder;
        let value = "test string".to_string();
        let encoded: BytesView = Encoder::<String, BytesView>::encode(&encoder, &value).unwrap();
        assert!(encoded.first_slice().len() > 0, "BincodeEncoder should produce non-empty output");
        // Verify the encoded data can be decoded by BincodeCodec
        let codec = BincodeCodec;
        let decoded: String = Codec::<String, BytesView>::decode(&codec, &encoded).unwrap();
        assert_eq!(decoded, value);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn serialize_builder_without_compress_or_encrypt() {
        use cachet::{BincodeEncoder, Cache};
        use tick::Clock;

        let clock = Clock::new_frozen();
        let remote = Cache::builder::<BytesView, BytesView>(clock.clone()).storage(MockCache::new());

        let cache = Cache::builder::<String, UserProfile>(clock)
            .storage(MockCache::new())
            .serialize(BincodeEncoder, BincodeCodec)
            .fallback(remote)
            .build();

        let profile = UserProfile {
            name: "Charlie".into(),
            age: 40,
            tags: vec![],
        };
        cache.insert("key1".to_string(), CacheEntry::new(profile.clone())).await.unwrap();

        let result = cache.get(&"key1".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), profile);
    }
}

#[cfg(feature = "serialize")]
mod fallback_serialize_tests {
    use cachet::{BincodeCodec, BincodeEncoder, BytesView, Cache, CacheEntry, MockCache};
    use tick::Clock;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestStruct {
        name: String,
        value: u64,
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_builder_serialize() {
        let clock = Clock::new_frozen();

        // Post-serialize remote tier (bytes)
        let remote = Cache::builder::<BytesView, BytesView>(clock.clone()).storage(MockCache::new());

        // L2 memory tier (same types as outer)
        let l2 = Cache::builder::<String, TestStruct>(clock.clone()).storage(MockCache::new());

        // Build: L1 -> fallback L2 -> .serialize() -> fallback remote
        let cache = Cache::builder::<String, TestStruct>(clock)
            .storage(MockCache::new())
            .fallback(l2)
            .serialize(BincodeEncoder, BincodeCodec)
            .fallback(remote)
            .build();

        let item = TestStruct {
            name: "test".into(),
            value: 42,
        };
        cache.insert("key1".to_string(), CacheEntry::new(item.clone())).await.unwrap();

        let result = cache.get(&"key1".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), item);
    }
}
