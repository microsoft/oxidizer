// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `CacheEntry`.

use std::time::{Duration, Instant};

use cachelon_tier::CacheEntry;

#[test]
fn new_creates_entry_without_timestamp() {
    let entry = CacheEntry::new("test_value");
    assert_eq!(*entry.value(), "test_value");
    assert!(entry.cached_at().is_none());
    assert!(entry.ttl().is_none());
}

#[test]
fn with_ttl_creates_entry_with_ttl() {
    let ttl = Duration::from_secs(300);
    let entry = CacheEntry::with_ttl("value", ttl);
    assert_eq!(*entry.value(), "value");
    assert_eq!(entry.ttl(), Some(ttl));
    assert!(entry.cached_at().is_none());
}

#[test]
fn with_cached_at_creates_entry_with_timestamp() {
    let now = Instant::now();
    let entry = CacheEntry::with_cached_at("value", now);
    assert_eq!(*entry.value(), "value");
    assert_eq!(entry.cached_at(), Some(now));
    assert!(entry.ttl().is_none());
}

#[test]
fn set_cached_at_updates_timestamp() {
    let mut entry = CacheEntry::new("value");
    assert!(entry.cached_at().is_none());

    let now = Instant::now();
    entry.set_cached_at(now);
    assert_eq!(entry.cached_at(), Some(now));
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
fn debug_includes_value() {
    let entry = CacheEntry::new(42);
    let debug_str = format!("{entry:?}");
    assert!(debug_str.contains("42"));
}
