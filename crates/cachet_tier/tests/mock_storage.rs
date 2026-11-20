// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mock storage through public APIs.

#![cfg(test)]

#[cfg(feature = "test-util")]
mod testing {
    use cachet_tier::{CacheEntry, CacheOp, CacheTier, MockCache};

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn insert_failure() {
        let cache = MockCache::<String, i32>::new();
        cache.fail_when(|op| matches!(op, CacheOp::Insert { .. }));
        let result = cache.insert("key".to_string(), CacheEntry::new(42)).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn invalidate_failure() {
        let cache = MockCache::<String, i32>::new();
        cache.fail_when(|op| matches!(op, CacheOp::Invalidate(_)));
        let result = cache.invalidate(&"key".to_string()).await;
        result.unwrap_err();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn clear_failure() {
        let cache = MockCache::<String, i32>::new();
        cache.fail_when(|op| matches!(op, CacheOp::Clear));
        let result = cache.clear().await;
        result.unwrap_err();
    }
}
