// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the arena-backed `hashbrown` collection builders, gated on the
//! `hashbrown` feature. They cover construction plus enough churn to exercise
//! the `&Arena` legacy-allocator path under table growth.

#![cfg(feature = "hashbrown")]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use multitude::Arena;

#[test]
fn hash_map_grows_across_chunks() {
    let arena = Arena::new();
    let mut map = arena.alloc_hash_map::<u32, std::string::String>();
    assert!(map.is_empty());
    for i in 0..256_u32 {
        map.insert(i, std::format!("v{i}"));
    }
    assert_eq!(map.len(), 256);
    assert_eq!(map.get(&42).map(std::string::String::as_str), Some("v42"));
    assert_eq!(map.get(&256), None);
    map.remove(&42);
    assert_eq!(map.get(&42), None);
    assert_eq!(map.len(), 255);
}

#[test]
fn hash_map_with_capacity_preallocates() {
    let arena = Arena::new();
    let mut map = arena.alloc_hash_map_with_capacity::<u32, u32>(128);
    assert!(map.capacity() >= 128);
    map.insert(1, 10);
    assert_eq!(map.get(&1), Some(&10));
}

#[test]
fn hash_set_grows_across_chunks() {
    let arena = Arena::new();
    let mut set = arena.alloc_set::<u32>();
    assert!(set.is_empty());
    for i in 0..256_u32 {
        set.insert(i);
    }
    assert_eq!(set.len(), 256);
    assert!(set.contains(&255));
    assert!(!set.contains(&256));
    set.remove(&255);
    assert!(!set.contains(&255));
}

#[test]
fn hash_set_with_capacity_preallocates() {
    let arena = Arena::new();
    let mut set = arena.alloc_set_with_capacity::<u32>(128);
    assert!(set.capacity() >= 128);
    set.insert(7);
    assert!(set.contains(&7));
}
