// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Notifications emitted by the in-memory cache.

/// The reason an entry was removed from the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RemovalCause {
    /// The entry's TTL or TTI passed and the cache's background maintenance
    /// reaped it. This is distinct from a get-time expiration check.
    Expired,

    /// The cache was at capacity and the eviction policy selected this entry
    /// for removal.
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
