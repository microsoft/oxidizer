// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the Serialization API.

#![cfg(feature = "serialize")]

use bytesbuf::BytesView;
use cachet::{Cache, CacheEntry, CacheOp, MockCache};
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn encoder_encode_encodes_correctly() {
    let mock_cache_before = MockCache::<String, String>::new();
    let mock_cache_after = Cache::builder(Clock::new_frozen()).storage(MockCache::<BytesView, BytesView>::new());
    let cache = Cache::builder(Clock::new_frozen())
        .storage(mock_cache_before)
        .serialize()
        .fallback(mock_cache_after)
        .build();
    let key = "greeting".to_string();
    let expected_value = "Hello, world!".to_string();
    cache
        .insert(key.clone(), expected_value.clone().into())
        .await
        .expect("Insert failed");
    let actual_value = cache.get(&key).await.expect("Get failed");

    assert_eq!(
        mock_cache_before.operations(),
        vec![
            CacheOp::Insert {
                key: "greeting".to_string(),
                entry: CacheEntry::new("Hello, world!".to_string())
            },
            CacheOp::Get("greeting".to_string()),
        ]
    );

    assert_eq!(
        mock_cache_after.operations(),
        vec![
            CacheOp::Insert {
                key: BytesView::from("greeting".as_bytes()),
                entry: CacheEntry::new(BytesView::from("Hello, world!".as_bytes()))
            },
            CacheOp::Get(BytesView::from("greeting".as_bytes())),
        ]
    );

    assert_eq!(actual_value.as_deref(), Some(expected_value).as_ref());
}
