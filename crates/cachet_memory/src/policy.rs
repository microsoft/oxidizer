// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Eviction policy configuration for the in-memory cache.

use std::fmt;

/// The eviction policy used by [`InMemoryCache`](crate::InMemoryCache) when the
/// cache reaches its maximum capacity.
///
/// # Policies
///
/// - **`TinyLFU`** (default): Combines frequency and recency for excellent hit rates.
///   Best for most workloads - keeps frequently accessed items even if not recently used.
/// - **LRU**: Evicts the least recently used entry. Simpler and more predictable,
///   but may evict frequently accessed items that haven't been touched recently.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EvictionPolicy {
    pub(crate) inner: EvictionPolicyInner,
}

impl EvictionPolicy {
    /// Creates a `TinyLFU` eviction policy (the default).
    ///
    /// `TinyLFU` combines frequency and recency tracking to achieve high cache
    /// hit rates across a wide range of workloads.
    #[must_use]
    pub fn tiny_lfu() -> Self {
        Self {
            inner: EvictionPolicyInner::TinyLfu,
        }
    }

    /// Creates an LRU (Least Recently Used) eviction policy.
    ///
    /// LRU evicts the entry that was accessed least recently. This is simpler
    /// than `TinyLFU` and may be preferred when access patterns are highly temporal
    /// (e.g., streaming or scanning workloads).
    #[must_use]
    pub fn lru() -> Self {
        Self {
            inner: EvictionPolicyInner::Lru,
        }
    }

    pub(crate) fn into_moka_policy(self) -> moka::policy::EvictionPolicy {
        match self.inner {
            EvictionPolicyInner::TinyLfu => moka::policy::EvictionPolicy::tiny_lfu(),
            EvictionPolicyInner::Lru => moka::policy::EvictionPolicy::lru(),
        }
    }
}

impl fmt::Debug for EvictionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner {
            EvictionPolicyInner::TinyLfu => write!(f, "EvictionPolicy::TinyLfu"),
            EvictionPolicyInner::Lru => write!(f, "EvictionPolicy::Lru"),
        }
    }
}

impl fmt::Display for EvictionPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner {
            EvictionPolicyInner::TinyLfu => write!(f, "TinyLFU"),
            EvictionPolicyInner::Lru => write!(f, "LRU"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum EvictionPolicyInner {
    #[default]
    TinyLfu,
    Lru,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_lfu_policy_has_correct_inner() {
        let policy = EvictionPolicy::tiny_lfu();
        assert_eq!(policy.inner, EvictionPolicyInner::TinyLfu);
    }

    #[test]
    fn lru_policy_has_correct_inner() {
        let policy = EvictionPolicy::lru();
        assert_eq!(policy.inner, EvictionPolicyInner::Lru);
    }

    #[test]
    fn default_policy_is_tiny_lfu() {
        let policy = EvictionPolicy::default();
        assert_eq!(policy.inner, EvictionPolicyInner::TinyLfu);
    }

    #[test]
    fn into_moka_policy() {
        let tiny_lfu_policy = EvictionPolicy::tiny_lfu();
        let expected_moka_policy = moka::policy::EvictionPolicy::tiny_lfu();
        let actual_moka_policy = tiny_lfu_policy.into_moka_policy();
        assert_eq!(format!("{actual_moka_policy:?}"), format!("{expected_moka_policy:?}"));

        let lru_policy = EvictionPolicy::lru();
        let expected_moka_policy = moka::policy::EvictionPolicy::lru();
        let actual_moka_policy = lru_policy.into_moka_policy();
        assert_eq!(format!("{actual_moka_policy:?}"), format!("{expected_moka_policy:?}"));
    }
}
