// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `CacheWrapper` public API (through Cache).

#![cfg(feature = "memory")]

use std::ops::Add;
use std::time::Duration;

use cachet::{Cache, CacheEntry};
use cachet_tier::{CacheOp, MockCache};
use tick::{Clock, ClockControl};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_name() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();
    let wrapper = cache.inner();
    assert!(!wrapper.name().is_empty());
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_get_miss() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let result = cache.get(&"nonexistent".to_string()).await.unwrap();
        assert!(result.is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_get_hit() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();

        let result = cache.get(&key).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_insert() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();

        assert!(cache.get(&key).await.unwrap().is_some());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_invalidate() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();
        cache.invalidate(&key).await.unwrap();

        assert!(cache.get(&key).await.unwrap().is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_clear() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        cache.insert(&"k1".to_string(), CacheEntry::new(1)).await.unwrap();
        cache.insert(&"k2".to_string(), CacheEntry::new(2)).await.unwrap();

        cache.clear().await.unwrap();

        assert!(cache.get(&"k1".to_string()).await.unwrap().is_none());
        assert!(cache.get(&"k2".to_string()).await.unwrap().is_none());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_len_returns_correct_count() {
    block_on(async {
        // Use MockCache for immediate consistency of len()
        let clock = Clock::new_frozen();
        let cache = Cache::builder(clock).storage(MockCache::<String, i32>::new()).build();

        assert_eq!(cache.len(), Some(0));

        cache.insert(&"key".to_string(), CacheEntry::new(42)).await.unwrap();

        assert_eq!(cache.len(), Some(1));
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_with_ttl_configured() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().ttl(Duration::from_secs(60)).build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();

        // Entry should exist immediately after insertion
        let result = cache.get(&key).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_entry_with_ttl() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        // Entry with per-entry TTL
        let entry = CacheEntry::expires_after(42, Duration::from_secs(120));
        cache.insert(&key, entry).await.unwrap();

        // Entry should exist immediately after insertion
        let result = cache.get(&key).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_no_ttl_configured() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock).memory().build();

        let key = "key".to_string();
        cache.insert(&key, CacheEntry::new(42)).await.unwrap();

        // Entry should exist (no TTL configured)
        let result = cache.get(&key).await.unwrap();
        assert!(result.is_some());
        assert_eq!(*result.unwrap().value(), 42);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_get_error_is_recorded() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        mock.fail_when(|op| matches!(op, CacheOp::Get(_)));
        let cache = Cache::builder(clock).storage(mock).build();

        let result = cache.get(&"key".to_string()).await;
        result.unwrap_err();
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_insert_error_is_recorded() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        mock.fail_when(|op| matches!(op, CacheOp::Insert { .. }));
        let cache = Cache::builder(clock).storage(mock).build();

        let result = cache.insert(&"key".to_string(), CacheEntry::new(42)).await;
        assert!(result.is_err());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_invalidate_error_is_recorded() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        mock.fail_when(|op| matches!(op, CacheOp::Invalidate(_)));
        let cache = Cache::builder(clock).storage(mock).build();

        let result = cache.invalidate(&"key".to_string()).await;
        assert!(result.is_err());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_clear_error_is_recorded() {
    block_on(async {
        let clock = Clock::new_frozen();
        let mock = MockCache::<String, i32>::new();
        mock.fail_when(|op| matches!(op, CacheOp::Clear));
        let cache = Cache::builder(clock).storage(mock).build();

        let result = cache.clear().await;
        assert!(result.is_err());
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_expired_entry_returns_none() {
    block_on(async {
        // Use a frozen clock so we control time precisely
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock.clone())
            .memory()
            .ttl(Duration::from_secs(60))
            .build();

        let key = "key".to_string();

        // Insert an entry with a cached_at in the far past so it is expired
        let entry = CacheEntry::expires_at(42, Duration::from_secs(1), clock.system_time() - Duration::from_secs(100));
        cache.insert(&key, entry).await.unwrap();

        // Entry should be treated as expired
        let result = cache.get(&key).await.unwrap();
        assert!(result.is_none(), "expired entry should return None");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_inner_returns_reference() {
    let clock = Clock::new_frozen();
    let cache = Cache::builder::<String, i32>(clock).memory().build();
    // inner() on the CacheWrapper
    let _inner = cache.inner().inner();
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_entry_expired_by_tier_ttl_without_per_entry_ttl() {
    block_on(async {
        // Entry has no per-entry TTL, but tier TTL is very short and entry is old
        let control = ClockControl::new();
        let clock = control.to_clock();
        let ttl = Duration::from_secs(1);
        let cache = Cache::builder::<String, i32>(clock.clone()).memory().ttl(ttl).build();

        let key = "key".to_string();

        // Insert an entry that looks very old (no per-entry TTL, just cached_at in the past)
        let entry = CacheEntry::new(42);
        cache.insert(&key, entry).await.unwrap();

        control.advance(ttl.add(Duration::from_secs(1)));
        let result = cache.get(&key).await.unwrap();
        assert!(result.is_none(), "entry expired by tier TTL should return None");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn wrapper_tier_ttl_expires_entry_without_per_entry_ttl() {
    block_on(async {
        let clock = Clock::new_frozen();
        let cache = Cache::builder::<String, i32>(clock.clone())
            .memory()
            .ttl(Duration::from_secs(10))
            .build();

        let key = "key".to_string();

        // Entry with no per-entry TTL, but cached_at pre-set to far in the past
        let mut entry = CacheEntry::new(42);
        entry.ensure_cached_at(clock.system_time() - Duration::from_secs(100));
        cache.insert(&key, entry).await.unwrap();

        // Tier TTL of 10s should expire this entry (100s old)
        let result = cache.get(&key).await.unwrap();
        assert!(result.is_none(), "entry should be expired by tier TTL alone");
    });
}
