// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Notifications emitted by the in-memory cache.
//!
//! Currently, this module exposes [`RemovalCause`] which classifies why an
//! entry was removed from the cache. It is delivered to listeners registered
//! via [`InMemoryCacheBuilder::on_eviction`](crate::InMemoryCacheBuilder::on_eviction).

/// The reason an entry was removed from the cache.
///
/// Mirrors moka crate's removal-cause classification, kept as a local type so that
/// the underlying cache implementation is not exposed in the public API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RemovalCause {
    /// The entry's TTL or TTI passed and it was removed by the cache.
    ///
    /// This is distinct from a get-time expiration check: this variant fires
    /// when the cache's background maintenance proactively reaps the entry.
    Expired,

    /// The entry was removed because the cache was at capacity and the
    /// eviction policy selected it for removal.
    Size,

    /// The entry was removed by an explicit
    /// [`invalidate`](cachet_tier::CacheTier::invalidate) or
    /// [`clear`](cachet_tier::CacheTier::clear) call.
    Explicit,

    /// The entry's value was replaced by a subsequent
    /// [`insert`](cachet_tier::CacheTier::insert) with the same key.
    Replaced,
}

pub(crate) fn from_moka(cause: moka::notification::RemovalCause) -> RemovalCause {
    match cause {
        moka::notification::RemovalCause::Expired => RemovalCause::Expired,
        moka::notification::RemovalCause::Size => RemovalCause::Size,
        moka::notification::RemovalCause::Explicit => RemovalCause::Explicit,
        moka::notification::RemovalCause::Replaced => RemovalCause::Replaced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_moka_maps_all_variants() {
        assert_eq!(from_moka(moka::notification::RemovalCause::Expired), RemovalCause::Expired);
        assert_eq!(from_moka(moka::notification::RemovalCause::Size), RemovalCause::Size);
        assert_eq!(from_moka(moka::notification::RemovalCause::Explicit), RemovalCause::Explicit);
        assert_eq!(from_moka(moka::notification::RemovalCause::Replaced), RemovalCause::Replaced);
    }
}
