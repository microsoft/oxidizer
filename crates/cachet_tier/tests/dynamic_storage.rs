// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Dynamic storage through public APIs.

#![cfg(test)]

#[cfg(feature = "test-util")]
mod dynamic {
    use cachet_tier::{CacheEntry, CacheTier, DynamicCache, InsertOutcome, MockCache};

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn clone_shares_state() {
        let cache = MockCache::<String, i32>::new();
        let dynamic = DynamicCache::new(cache);
        let clone = dynamic.clone();

        let outcome = dynamic.insert("key".to_string(), CacheEntry::new(42)).await.unwrap();
        assert_eq!(outcome, InsertOutcome::Accepted);

        let entry = clone.get(&"key".to_string()).await.unwrap().unwrap();
        assert_eq!(*entry.value(), 42);
    }
}
