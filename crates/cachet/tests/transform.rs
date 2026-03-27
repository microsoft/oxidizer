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
// BincodeCodec round-trip tests (serialize feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "serialize")]
mod serialize_tests {
    use cachet::{BincodeCodec, BytesView, CacheEntry, Codec, Encoder, MockCache};

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

// ---------------------------------------------------------------------------
// ZstdCodec round-trip tests (compress feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "compress")]
mod compress_tests {
    use cachet::{BytesView, Codec, Encoder, ZstdCodec};

    fn make_bytes(data: &[u8]) -> BytesView {
        Vec::from(data).into()
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn zstd_round_trip_small_data() {
        let codec = ZstdCodec::new(3);
        let original = make_bytes(b"small payload");
        let compressed = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decompressed = Codec::<BytesView, BytesView>::decode(&codec, &compressed).unwrap();
        assert_eq!(decompressed, original.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn zstd_round_trip_large_data() {
        let codec = ZstdCodec::new(3);
        let data: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();
        let original = make_bytes(&data);
        let compressed = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decompressed = Codec::<BytesView, BytesView>::decode(&codec, &compressed).unwrap();
        assert_eq!(decompressed, original.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn zstd_round_trip_empty_bytes() {
        let codec = ZstdCodec::new(3);
        let original = make_bytes(b"");
        let compressed = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decompressed = Codec::<BytesView, BytesView>::decode(&codec, &compressed).unwrap();
        assert_eq!(decompressed, original.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn zstd_compression_level_1() {
        let codec = ZstdCodec::new(1);
        let original = make_bytes(b"test data for level 1 compression");
        let compressed = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decompressed = Codec::<BytesView, BytesView>::decode(&codec, &compressed).unwrap();
        assert_eq!(decompressed, original.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn zstd_compression_level_22() {
        let codec = ZstdCodec::new(22);
        let original = make_bytes(b"test data for level 22 compression");
        let compressed = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decompressed = Codec::<BytesView, BytesView>::decode(&codec, &compressed).unwrap();
        assert_eq!(decompressed, original.first_slice());
    }
}

// ---------------------------------------------------------------------------
// AesGcmCodec round-trip tests (encrypt feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "encrypt")]
mod encrypt_tests {
    use cachet::{AesGcmCodec, BytesView, Codec, Encoder};

    fn make_bytes(data: &[u8]) -> BytesView {
        Vec::from(data).into()
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn aesgcm_round_trip_small_data() {
        let key = [1u8; 32];
        let codec = AesGcmCodec::new(&key);
        let original = make_bytes(b"hello encryption");
        let encrypted = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decrypted = Codec::<BytesView, BytesView>::decode(&codec, &encrypted).unwrap();
        assert_eq!(decrypted, original.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn aesgcm_round_trip_large_data() {
        let key = [2u8; 32];
        let codec = AesGcmCodec::new(&key);
        let data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
        let original = make_bytes(&data);
        let encrypted = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decrypted = Codec::<BytesView, BytesView>::decode(&codec, &encrypted).unwrap();
        assert_eq!(decrypted, original.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn aesgcm_round_trip_empty_bytes() {
        let key = [3u8; 32];
        let codec = AesGcmCodec::new(&key);
        let original = make_bytes(b"");
        let encrypted = Encoder::<BytesView, BytesView>::encode(&codec, &original).unwrap();
        let decrypted = Codec::<BytesView, BytesView>::decode(&codec, &encrypted).unwrap();
        assert_eq!(decrypted, original.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn aesgcm_different_keys_produce_different_ciphertext() {
        let key_a = [10u8; 32];
        let key_b = [20u8; 32];
        let codec_a = AesGcmCodec::new(&key_a);
        let codec_b = AesGcmCodec::new(&key_b);
        let original = make_bytes(b"same plaintext");

        let encrypted_a = Encoder::<BytesView, BytesView>::encode(&codec_a, &original).unwrap();
        let encrypted_b = Encoder::<BytesView, BytesView>::encode(&codec_b, &original).unwrap();

        // Ciphertext must differ (different keys + random nonces)
        assert_ne!(encrypted_a.first_slice(), encrypted_b.first_slice());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn aesgcm_wrong_key_fails_to_decrypt() {
        let key_a = [10u8; 32];
        let key_b = [20u8; 32];
        let codec_a = AesGcmCodec::new(&key_a);
        let codec_b = AesGcmCodec::new(&key_b);
        let original = make_bytes(b"secret data");

        let encrypted = Encoder::<BytesView, BytesView>::encode(&codec_a, &original).unwrap();
        let result = Codec::<BytesView, BytesView>::decode(&codec_b, &encrypted);
        assert!(result.is_err());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn aesgcm_truncated_ciphertext_fails() {
        let key = [5u8; 32];
        let codec = AesGcmCodec::new(&key);
        // Fewer than 12 bytes → nonce is missing
        let truncated = make_bytes(&[0u8; 5]);
        let result = Codec::<BytesView, BytesView>::decode(&codec, &truncated);
        assert!(result.is_err());
    }
}

// ---------------------------------------------------------------------------
// Chained codec tests (serialize + compress)
// ---------------------------------------------------------------------------

#[cfg(all(feature = "serialize", feature = "compress"))]
mod chained_tests {
    use cachet::{BincodeCodec, BincodeEncoder, BytesView, Cache, CacheEntry, MockCache, ZstdCodec};
    use tick::Clock;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Item {
        id: u64,
        label: String,
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn serialize_then_compress_round_trip() {
        let clock = Clock::new_frozen();
        let remote = Cache::builder::<BytesView, BytesView>(clock.clone()).storage(MockCache::new());

        let cache = Cache::builder::<String, Item>(clock)
            .storage(MockCache::new())
            .serialize(BincodeEncoder, BincodeCodec)
            .compress(ZstdCodec::new(3))
            .fallback(remote)
            .build();

        let item = Item {
            id: 42,
            label: "test-item".into(),
        };
        cache.insert("k1".to_string(), CacheEntry::new(item.clone())).await.unwrap();

        let result = cache.get(&"k1".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), item);
    }
}

// ---------------------------------------------------------------------------
// Full pipeline tests (serialize + compress + encrypt)
// ---------------------------------------------------------------------------

#[cfg(all(feature = "serialize", feature = "compress", feature = "encrypt"))]
mod full_pipeline_tests {
    use cachet::{AesGcmCodec, BincodeCodec, BincodeEncoder, BytesView, Cache, CacheEntry, DynamicCache, MockCache, ZstdCodec};
    use tick::Clock;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Order {
        order_id: u64,
        product: String,
        quantity: u32,
    }

    fn build_full_pipeline(clock: Clock) -> Cache<String, Order, DynamicCache<String, Order>> {
        let key = [42u8; 32];
        let remote = Cache::builder::<BytesView, BytesView>(clock.clone()).storage(MockCache::new());

        Cache::builder::<String, Order>(clock)
            .storage(MockCache::new())
            .serialize(BincodeEncoder, BincodeCodec)
            .compress(ZstdCodec::new(3))
            .encrypt(AesGcmCodec::new(&key))
            .fallback(remote)
            .build()
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn full_pipeline_insert_and_get() {
        let clock = Clock::new_frozen();
        let cache = build_full_pipeline(clock);

        let order = Order {
            order_id: 1001,
            product: "widget".into(),
            quantity: 5,
        };
        cache.insert("order-1".to_string(), CacheEntry::new(order.clone())).await.unwrap();

        let result = cache.get(&"order-1".to_string()).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), order);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn full_pipeline_invalidate() {
        let clock = Clock::new_frozen();
        let cache = build_full_pipeline(clock);

        let order = Order {
            order_id: 2002,
            product: "gadget".into(),
            quantity: 3,
        };
        cache.insert("order-2".to_string(), CacheEntry::new(order)).await.unwrap();
        cache.invalidate(&"order-2".to_string()).await.unwrap();

        let result = cache.get(&"order-2".to_string()).await.unwrap();
        assert!(result.is_none());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn full_pipeline_clear() {
        let clock = Clock::new_frozen();
        let cache = build_full_pipeline(clock);

        let order = Order {
            order_id: 3003,
            product: "doohickey".into(),
            quantity: 1,
        };
        cache.insert("order-3".to_string(), CacheEntry::new(order)).await.unwrap();
        cache.clear().await.unwrap();

        let len = cache.len().await.unwrap();
        assert_eq!(len, Some(0));
    }
}
