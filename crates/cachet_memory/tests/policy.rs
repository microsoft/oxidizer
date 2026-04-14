// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `EvictionPolicy`.

use cachet_memory::policy::EvictionPolicy;

#[test]
fn tiny_lfu_policy_debug() {
    let policy = EvictionPolicy::tiny_lfu();
    assert_eq!(format!("{policy:?}"), "EvictionPolicy::TinyLfu");
}

#[test]
fn lru_policy_debug() {
    let policy = EvictionPolicy::lru();
    assert_eq!(format!("{policy:?}"), "EvictionPolicy::Lru");
}

#[test]
fn tiny_lfu_policy_display() {
    let policy = EvictionPolicy::tiny_lfu();
    assert_eq!(format!("{policy}"), "TinyLFU");
}

#[test]
fn lru_policy_display() {
    let policy = EvictionPolicy::lru();
    assert_eq!(format!("{policy}"), "LRU");
}

#[test]
fn default_policy_is_tiny_lfu() {
    let policy = EvictionPolicy::default();
    assert_eq!(policy, EvictionPolicy::tiny_lfu());
}
