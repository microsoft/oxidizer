// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Eviction policy through public APIs.

#![cfg(test)]

mod policy {
    use cachet_memory::policy::EvictionPolicy;

    #[test]
    fn tiny_lfu_policy_has_correct_inner() {
        let policy = EvictionPolicy::tiny_lfu();
        assert_eq!(format!("{policy:?}"), "EvictionPolicy::TinyLfu");
    }

    #[test]
    fn lru_policy_has_correct_inner() {
        let policy = EvictionPolicy::lru();
        assert_eq!(format!("{policy:?}"), "EvictionPolicy::Lru");
    }

    #[test]
    fn default_policy_is_tiny_lfu() {
        let policy = EvictionPolicy::default();
        assert_eq!(policy, EvictionPolicy::tiny_lfu());
    }
}
