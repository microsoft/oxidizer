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
    #[must_use]
    pub fn serialize(self) -> TransformBuilder<K, V, BytesView, BytesView, Self>
    where
        K: Serialize,
        V: Serialize + DeserializeOwned,
    {
        self.transform(BincodeEncoder, BincodeCodec)
    }
}

// ── .serialize() on FallbackBuilder ──

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
