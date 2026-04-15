// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builder methods for serialization of cache Key and Value.

use bytesbuf::BytesView;
use cachet_tier::CacheTier;
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    CacheBuilder, CacheTierBuilder, FallbackBuilder, TransformBuilder,
    transform::{BincodeCodec, BincodeEncoder},
};

impl<K, V, CT> CacheBuilder<K, V, CT>
where
    K: Send + Sync + 'static,
    V: Send + Sync + 'static,
    CT: CacheTier<K, V> + Send + Sync + 'static,
{
    /// Applies a serialization boundary that converts keys and values to [`BytesView`](bytesbuf::BytesView).
    ///
    /// Subsequent `.fallback()` tiers must work with `BytesView` keys and values
    /// (i.e., implement [`DistributedCacheTier`](cachet_tier::DistributedCacheTier)).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use cachet::{Cache, FallbackPromotionPolicy};
    /// use tick::Clock;
    ///
    /// let clock = Clock::new_tokio();
    /// let remote = Cache::builder::<bytesbuf::BytesView, bytesbuf::BytesView>(clock.clone()).memory();
    ///
    /// let cache = Cache::builder::<String, String>(clock)
    ///     .memory()
    ///     .serialize()
    ///     .fallback(remote)
    ///     .promotion_policy(FallbackPromotionPolicy::always())
    ///     .build();
    /// ```
    #[must_use]
    pub fn serialize(self) -> TransformBuilder<K, V, BytesView, BytesView, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        self.transform(BincodeEncoder, BincodeCodec)
    }
}

impl<K, V, PB, FB> FallbackBuilder<K, V, PB, FB>
where
    K: Send + Sync + 'static,
    V: Send + Sync + 'static,
    PB: CacheTierBuilder<K, V>,
    FB: CacheTierBuilder<K, V>,
{
    /// Applies a serialization boundary on a fallback builder.
    #[must_use]
    pub fn serialize(self) -> TransformBuilder<K, V, BytesView, BytesView, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        self.transform(BincodeEncoder, BincodeCodec)
    }
}
