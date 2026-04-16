// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `CacheEntry`.

use std::time::Duration;

use cachet_tier::CacheEntry;
use tick::Clock;

#[test]
fn new_creates_entry_without_timestamp() {
    let entry = CacheEntry::new("test_value");
    assert_eq!(*entry.value(), "test_value");
    assert!(entry.cached_at().is_none());
    assert!(entry.ttl().is_none());
}

#[test]
fn expires_after_creates_entry_with_ttl() {
    let ttl = Duration::from_secs(300);
    let entry = CacheEntry::expires_after("value", ttl);
    assert_eq!(*entry.value(), "value");
    assert_eq!(entry.ttl(), Some(ttl));
    assert!(entry.cached_at().is_none());
}

#[test]
fn expires_at_creates_entry_with_ttl_and_timestamp() {
    let now = Clock::new_frozen().system_time();
    let ttl = Duration::from_secs(300);
    let entry = CacheEntry::expires_at("value", ttl, now);
    assert_eq!(*entry.value(), "value");
    assert_eq!(entry.cached_at(), Some(now));
    assert_eq!(entry.ttl(), Some(ttl));
}

#[test]
fn set_ttl_updates_ttl() {
    let mut entry = CacheEntry::new("value");
    assert!(entry.ttl().is_none());

    let ttl = Duration::from_secs(60);
    entry.set_ttl(ttl);
    assert_eq!(entry.ttl(), Some(ttl));
}

#[test]
fn into_value_consumes_entry() {
    let entry = CacheEntry::new("owned_value".to_string());
    let value = entry.into_value();
    assert_eq!(value, "owned_value");
}

#[test]
fn deref_returns_value_reference() {
    let entry = CacheEntry::new(42i32);
    let val: &i32 = &entry;
    assert_eq!(*val, 42);
}

#[test]
fn from_creates_entry_from_value() {
    let entry: CacheEntry<String> = "test".to_string().into();
    assert_eq!(*entry.value(), "test");
}

#[test]
fn clone_creates_identical_copy() {
    let entry = CacheEntry::new("value".to_string());
    let cloned = entry.clone();
    assert_eq!(entry.value(), cloned.value());
}

#[test]
fn ensure_cached_at_sets_timestamp_when_none() {
    let now = Clock::new_frozen().system_time();
    let mut entry = CacheEntry::new(42);
    assert!(entry.cached_at().is_none());

    entry.ensure_cached_at(now);
    assert_eq!(entry.cached_at(), Some(now));
}

#[test]
fn ensure_cached_at_preserves_existing_timestamp() {
    let original = Clock::new_frozen().system_time();
    let later = original + Duration::from_secs(100);
    let mut entry = CacheEntry::expires_at(42, Duration::from_secs(60), original);
    assert_eq!(entry.cached_at(), Some(original));

    // Should NOT overwrite the existing timestamp
    entry.ensure_cached_at(later);
    assert_eq!(entry.cached_at(), Some(original));
}

#[test]
fn debug_includes_value() {
    let entry = CacheEntry::new(42);
    let debug_str = format!("{entry:?}");
    assert!(debug_str.contains("42"));
}

#[test]
fn partial_eq_compares_value_and_ttl() {
    let a = CacheEntry::new(42);
    let b = CacheEntry::new(42);
    assert_eq!(a, b);
}

#[test]
fn partial_eq_ignores_cached_at() {
    let clock = Clock::new_frozen();
    let t1 = clock.system_time();
    let t2 = t1 + Duration::from_secs(100);

    let a = CacheEntry::expires_at(42, Duration::from_secs(60), t1);
    let b = CacheEntry::expires_at(42, Duration::from_secs(60), t2);

    // Same value and TTL but different cached_at — should be equal.
    assert_eq!(a, b);
}

#[test]
fn partial_eq_different_value() {
    let a = CacheEntry::new(42);
    let b = CacheEntry::new(99);
    assert_ne!(a, b);
}

#[test]
fn partial_eq_different_ttl() {
    let a = CacheEntry::expires_after(42, Duration::from_secs(60));
    let b = CacheEntry::expires_after(42, Duration::from_secs(300));
    assert_ne!(a, b);
}
