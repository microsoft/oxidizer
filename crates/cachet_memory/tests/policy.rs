// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for `EvictionPolicy`.

use cachet_memory::policy::EvictionPolicy;

#[test]
fn tiny_lfu_policy() {
    let policy = EvictionPolicy::tiny_lfu();
    assert_eq!(format!("{:?}", policy), "EvictionPolicy::TinyLfu");
}

#[test]
fn lru_policy() {
    let policy = EvictionPolicy::lru();
    assert_eq!(format!("{:?}", policy), "EvictionPolicy::Lru");
}

#[test]
fn default_policy_is_tiny_lfu() {
    let policy = EvictionPolicy::default();
    assert_eq!(format!("{:?}", policy), "EvictionPolicy::TinyLfu");
}
